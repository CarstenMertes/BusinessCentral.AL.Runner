#!/usr/bin/env bash
# Tests the wrapper on Linux with a fake al-runner: arguments (spaces, quotes, empty), stdin, exit
# codes, death by a signal, a forwarded SIGTERM and the ALRUNNER_NO_PATH_NOTE default. Runs both ways
# the wrapper starts the binary: execve (Linux and macOS) and a child process (Windows, and the fallback).
# Usage: packaging/dotnet-tool/test.sh [WORK_DIR]   (default: a new temporary folder)
# Needs the .NET 8 SDK or newer. Leaves native/, bin/ and obj/ next to this script (git-ignored).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
version=0.0.1-wrapper-test
rm -rf "$work/dist" "$work/stage" "$work/out" "$work/tools" "$work/packages"
mkdir -p "$work/dist" "$work/stage"

# The fake binary. "args" prints each argument in brackets, "cat" copies stdin, "exit N" exits with N,
# "kill SIG" kills itself, "wait" waits for SIGTERM, "env" prints ALRUNNER_NO_PATH_NOTE,
# "parent" prints the parent's command name (the wrapper only when it started a child process).
cat > "$work/stage/al-runner" <<'FAKE'
#!/usr/bin/env bash
case "$1" in
  args) shift; for a in "$@"; do printf '[%s]\n' "$a"; done ;;
  cat) cat ;;
  exit) exit "$2" ;;
  kill) kill -s "$2" $$ ;;
  wait) trap 'echo "got TERM"; exit 42' TERM; echo $$ > "$2"; while :; do sleep 0.1; done ;;
  env) echo "note=${ALRUNNER_NO_PATH_NOTE-unset}" ;;
  parent) ps -o comm= -p "$PPID" ;;
esac
FAKE
# The archives hold no execute bit for the fake, as NuGet would not keep it either.
chmod 644 "$work/stage/al-runner"
tar -C "$work/stage" -cJf "$work/dist/al-runner-$version-x86_64-unknown-linux-musl.tar.xz" ./al-runner
tar -C "$work/stage" -cJf "$work/dist/al-runner-$version-aarch64-apple-darwin.tar.xz" ./al-runner
cp "$work/stage/al-runner" "$work/stage/al-runner.exe"
(cd "$work/stage" && zip -q "$work/dist/al-runner-$version-x86_64-pc-windows-msvc.zip" al-runner.exe)

"$here/pack.sh" "$version" "$work/dist" "$work/out" > "$work/pack.log" || { cat "$work/pack.log"; exit 1; }
cat > "$work/nuget.config" <<CFG
<?xml version="1.0" encoding="utf-8"?>
<configuration><packageSources><clear /><add key="local" value="$work/out" /></packageSources></configuration>
CFG
export NUGET_PACKAGES="$work/packages"
dotnet tool install MSDyn365BC.AL.Runner --version "$version" --tool-path "$work/tools" --configfile "$work/nuget.config" > /dev/null
tool="$work/tools/al-runner"

fail=0
check() { # name expected actual
  if [ "$2" == "$3" ]; then echo "ok   $1"; else echo "FAIL $1: expected [$2], got [$3]"; fail=1; fi
}
run_checks() {
  local mode="$1" parent="$2"
  check "$mode: parent process" "$parent" "$("$tool" parent)"
  check "$mode: arguments" "$(printf '[a b]\n[]\n[c"d]\n[e'"'"'f]\n[*]\n[ü ß]\n[--x=1 2]')" \
    "$("$tool" args 'a b' '' 'c"d' "e'f" '*' 'ü ß' '--x=1 2')"
  check "$mode: stdin text" "$(printf 'line 1\nline 2')" "$(printf 'line 1\nline 2\n' | "$tool" cat)"
  head -c 200000 /dev/urandom > "$work/bytes"
  check "$mode: stdin bytes" "$(sha256sum < "$work/bytes")" "$("$tool" cat < "$work/bytes" | sha256sum)"
  for code in 0 1 2 3 7 255; do
    set +e; "$tool" exit "$code"; got=$?; set -e
    check "$mode: exit code $code" "$code" "$got"
  done
  set +e; "$tool" kill KILL; got=$?; set -e
  check "$mode: killed by SIGKILL" 137 "$got"
  rm -f "$work/ready"
  "$tool" wait "$work/ready" > "$work/wait.out" &
  local pid=$!
  local end=$(( $(date +%s) + 20 ))
  while [ ! -s "$work/ready" ] && [ "$(date +%s)" -lt "$end" ]; do sleep 0.1; done
  kill -TERM "$pid"
  set +e; wait "$pid"; got=$?; set -e
  check "$mode: SIGTERM to the wrapper reaches al-runner" "got TERM 42" "$(cat "$work/wait.out") $got"
  local fake
  fake="$(cat "$work/ready")"
  if kill -0 "$fake" 2>/dev/null; then
    kill -KILL "$fake"
    check "$mode: al-runner ends with the wrapper" "ended" "still running"
  fi
  check "$mode: path note off by default" "note=1" "$("$tool" env)"
  check "$mode: path note setting kept" "note=0" "$(ALRUNNER_NO_PATH_NOTE=0 "$tool" env)"
}
run_checks execve bash
ALRUNNER_DOTNET_TOOL_SPAWN=1 run_checks "child process" al-runner
[ "$fail" = 0 ] && echo "all wrapper checks passed" || { echo "wrapper checks failed"; exit 1; }
