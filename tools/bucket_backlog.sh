#!/usr/bin/env bash
# Runs every Microsoft test bucket (Tests-*.Source.zip) one after another and writes junit files,
# the console output per bucket and buckets.tsv into one directory. Then builds backlog.md/json.
#
#   tools/bucket_backlog.sh [OUT_DIR]
#
# OUT_DIR defaults to ~/.cache/al-runtime/buckets/<commit>/. Set PREVIOUS=<dir> to add the
# lost-pass section. Environment: ALRUNNER (binary, default ./target/release/alrunner),
# BUCKET_DIR (artifact folder), NO_SLICE=1 (do not wrap in systemd-run).
# Never run two of these at once (docs/agents/brief.md).
set -u
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
bin="${ALRUNNER:-$repo/target/release/alrunner}"
src="${BUCKET_DIR:-$HOME/.bcartifacts.cache/sandbox/27.5.46862.48827/platform/Applications/BaseApp/Test}"
commit="$(git -C "$repo" rev-parse --short HEAD)"
out="${1:-$HOME/.cache/al-runtime/buckets/$commit}"
mkdir -p "$out"
[ -x "$bin" ] || { echo "no binary at $bin (cargo build --release)" >&2; exit 2; }
"$bin" --version > "$out/version.txt" 2>&1
printf 'bucket\texit\tseconds\n' > "$out/buckets.tsv"
wrap=(systemd-run --user --scope -q --slice=al-work.slice -p MemoryMax=8G)
[ "${NO_SLICE:-}" = 1 ] && wrap=()
start=$SECONDS
shopt -s nullglob
for zip in "$src"/Tests-*.Source.zip; do
  b="$(basename "$zip" .Source.zip)"; b="${b#Tests-}"
  case "$b" in Local|TestLibraries) continue;; esac
  t0=$SECONDS
  "${wrap[@]}" "$bin" test "$zip" --junit "$out/$b.xml" > "$out/$b.console.txt" 2>&1
  code=$?
  printf '%s\t%s\t%s\n' "$b" "$code" "$((SECONDS - t0))" >> "$out/buckets.tsv"
  echo "bucket $b: exit $code, $((SECONDS - t0)) s: $(tail -n 1 "$out/$b.console.txt")"
done
echo "all buckets: $((SECONDS - start)) s" | tee "$out/total-time.txt"
args=("$out"); [ -n "${PREVIOUS:-}" ] && args+=(--previous "$PREVIOUS")
python3 "$here/bucket_backlog.py" "${args[@]}"
