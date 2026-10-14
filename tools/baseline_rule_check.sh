#!/usr/bin/env bash
# The rule that skips the install baseline (crates/al-testrun/src/observe.rs, ADR-013 decision 5
# note) must never change an outcome. A normal run only exercises it when every selected test
# passes the rule, so this check turns it on for each test codeunit separately: with
# AL_CHECK_BASELINE_RULE=1 the baseline runs as usual, but each test codeunit the rule calls free
# of it starts on an empty store. The JUnit files of that run must equal those of a normal run
# (zero differences, no allow list). The log of the second run says how many codeunits the rule
# covered, so a rule that stops matching anything cannot pass unnoticed.
#
#   tools/baseline_rule_check.sh compare DIR   compare the files in DIR
#   tools/baseline_rule_check.sh run [DIR]     make the runs, then compare
#
# File names in DIR, which is what `al-test-cli --all-versions --junit` writes:
#   normal:  runtime-junit-<v>.xml          runtime-junit-<suite>-<v>.xml
#   ruled:   ruled-junit-<v>.xml            ruled-junit-<suite>-<v>.xml
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
versions="27 28 29"

compare() {
  local dir="$1" status=0 n=0 f v partner
  for v in $versions; do
    echo "== BC $v"
    python3 "$here/compare_junit.py" "$dir/runtime-junit-$v.xml" "$dir/ruled-junit-$v.xml" || status=1
    n=$((n + 1))
  done
  for f in "$dir"/runtime-junit-*-*.xml; do
    [ -e "$f" ] || continue
    base="$(basename "$f" .xml)"
    partner="$dir/ruled-junit-${base#runtime-junit-}.xml"
    echo "== ${base#runtime-junit-}"
    if [ ! -f "$partner" ]; then
      echo "MISSING: $partner"
      status=1
      continue
    fi
    python3 "$here/compare_junit.py" "$f" "$partner" || status=1
    n=$((n + 1))
  done
  echo "compared $n pairs of JUnit files"
  [ -f "$dir/ruled.log" ] && grep "baseline rule check" "$dir/ruled.log"
  return $status
}

run() {
  local dir="$1" bin="$root/target/release/al-test-cli" status=0
  [ -x "$bin" ] || { echo "build first: cargo build --release -p al-test-cli" >&2; return 2; }
  mkdir -p "$dir"
  rm -f "$dir"/runtime-junit*.xml "$dir"/ruled-junit*.xml
  local cap=()
  command -v systemd-run >/dev/null && cap=(systemd-run --user --scope -q --slice=al-work.slice -p MemoryMax=3G)
  (cd "$dir" && "${cap[@]}" "$bin" --root "$root" --all-versions --require-dotnet --no-ratchet \
    --junit runtime-junit.xml > normal.log 2>&1) || { echo "normal run failed, see $dir/normal.log" >&2; status=1; }
  (cd "$dir" && AL_CHECK_BASELINE_RULE=1 "${cap[@]}" "$bin" --root "$root" --all-versions --require-dotnet --no-ratchet \
    --junit ruled-junit.xml > ruled.log 2>&1) || { echo "ruled run failed, see $dir/ruled.log" >&2; status=1; }
  compare "$dir" || status=1
  return $status
}

case "${1:-}" in
  compare) compare "${2:?usage: baseline_rule_check.sh compare DIR}" ;;
  run) run "${2:-$root/target/baseline-rule-check}" ;;
  *) sed -n '2,16p' "$0"; exit 2 ;;
esac
