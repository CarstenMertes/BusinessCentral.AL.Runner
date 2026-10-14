#!/usr/bin/env bash
# ADR-013 decision 6: the demand-driven compile of the dependency apps must never change an outcome.
# This compares the JUnit files of a normal corpus run with those of an eager run
# (AL_EAGER_DEPENDENCIES=1), for the main app and for every extra suite (OnPrem, the isolation
# probes). Any difference not listed in tools/demand-eager-known-differences.txt fails.
#
#   tools/demand_vs_eager.sh compare DIR   compare the files in DIR (what CI runs after its runs)
#   tools/demand_vs_eager.sh run [DIR]     make the runs, then compare (the local recipe; about 3 min)
#
# File names in DIR, which is what `al-test-cli --junit` writes:
#   normal, one `--all-versions --junit runtime-junit.xml` run:
#       runtime-junit-<v>.xml            runtime-junit-<suite>-<v>.xml
#   eager, one `--bc-version <v> --junit eager-junit-<v>.xml` run per version:
#       eager-junit-<v>.xml              eager-junit-<v>-<suite>.xml
# A normal file with no eager partner fails: a suite must not drop out of the comparison unseen.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
versions="27 28 29"

compare() {
  local dir="$1" status=0 n=0 f v suite partner
  for v in $versions; do
    python3 "$here/compare_junit.py" --allow "$here/demand-eager-known-differences.txt" \
      "$dir/runtime-junit-$v.xml" "$dir/eager-junit-$v.xml" || status=1
    n=$((n + 1))
  done
  # runtime-junit-<suite>-<v>.xml: the suites are the files whose name has more than the version.
  for f in "$dir"/runtime-junit-*-*.xml; do
    [ -e "$f" ] || continue
    base="$(basename "$f" .xml)"
    v="${base##*-}"
    suite="${base#runtime-junit-}"
    suite="${suite%-"$v"}"
    partner="$dir/eager-junit-$v-$suite.xml"
    echo "== BC $v suite $suite"
    if [ ! -f "$partner" ]; then
      echo "MISSING: $partner (the eager run wrote no file for suite $suite)"
      status=1
      continue
    fi
    python3 "$here/compare_junit.py" --allow "$here/demand-eager-known-differences.txt" \
      "$f" "$partner" || status=1
    n=$((n + 1))
  done
  echo "compared $n pairs of JUnit files"
  return $status
}

run() {
  local dir="$1" bin="$root/target/release/al-test-cli" status=0 v
  [ -x "$bin" ] || { echo "build first: cargo build --release -p al-test-cli" >&2; return 2; }
  mkdir -p "$dir"
  rm -f "$dir"/runtime-junit*.xml "$dir"/eager-junit*.xml
  # One corpus run at a time, under a memory cap (a run must stay under 3 GB).
  local cap=() eager_cap=()
  command -v systemd-run >/dev/null && cap=(systemd-run --user --scope -q --slice=al-work.slice -p MemoryMax=3G)
  # The eager compile holds every dependency routine and checks on all cores: 4 to 6 GB (docs/agents/brief.md).
  command -v systemd-run >/dev/null && eager_cap=(systemd-run --user --scope -q --slice=al-work.slice -p MemoryMax=6G)
  (cd "$dir" && "${cap[@]}" "$bin" --root "$root" --all-versions --require-dotnet --no-ratchet \
    --junit runtime-junit.xml > normal.log 2>&1) || { echo "normal run failed, see $dir/normal.log" >&2; status=1; }
  for v in $versions; do
    (cd "$dir" && AL_EAGER_DEPENDENCIES=1 "${eager_cap[@]}" "$bin" --root "$root" --bc-version "$v" --require-dotnet \
      --no-ratchet --junit "eager-junit-$v.xml" > "eager-$v.log" 2>&1) \
      || { echo "eager run for BC $v failed, see $dir/eager-$v.log" >&2; status=1; }
  done
  compare "$dir" || status=1
  return $status
}

case "${1:-}" in
  compare) compare "${2:?usage: demand_vs_eager.sh compare DIR}" ;;
  run) run "${2:-$root/target/demand-vs-eager}" ;;
  *) sed -n '2,15p' "$0"; exit 2 ;;
esac
