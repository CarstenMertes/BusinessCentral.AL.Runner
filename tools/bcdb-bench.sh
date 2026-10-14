#!/usr/bin/env bash
# Compares the published bcdb (C#, native AOT) with the Rust port (al-bcdb) on the same
# files and operations: describe, read one large table, read one small table, list the
# tables, and a full export (every table with rows, through the serve protocol, so both
# tools do the same work). Warm cache by default; --cold drops each input file from the
# page cache before every run (posix_fadvise DONTNEED, no root needed).
#
# Numbers taken while other builds run are not worth reporting: the load average is
# printed next to every row. Run this on a quiet machine.
#
#   tools/bcdb-bench.sh [--runs N] [--cold]
#
# Environment (all optional):
#   BCDB_REF      the reference bcdb      (default: bcdb on PATH, else the mise install)
#   BCDB_PORT     the Rust binary         (default: target/bcdb-bench/al-bcdb, built if missing)
#   BAK           a CRONUS .bak           (default: BC 28.1 W1 demo backup in the artifact cache)
#   BACPAC        a CRONUS .bacpac        (default: ~/.cache/al-runtime/bcdb-bench/cronus-28.1.bacpac)
#   BASEAPP       the Base Application .app for describe
#   COMPANY       company prefix to read  (default: CRONUS)
#
# The Rust side is built with: cargo build --profile bcdb-bench -p al-bcdb-cli
# (opt-level 3, fat LTO, codegen-units 1, panic=abort; no target-cpu flag).
set -euo pipefail

RUNS=7
COLD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) RUNS="$2"; shift 2 ;;
    --cold) COLD=1; shift ;;
    *) echo "unknown argument $1" >&2; exit 64 ;;
  esac
done

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CACHE=${BCDB_BENCH_CACHE:-$HOME/.cache/al-runtime/bcdb-bench}
mkdir -p "$CACHE"
REF=${BCDB_REF:-$(command -v bcdb || echo "$HOME/.local/share/mise/installs/dotnet-bcdb/latest/bin/bcdb")}
PORT=${BCDB_PORT:-$ROOT/target/bcdb-bench/al-bcdb}
V=28.1.49838.50621
ART=$HOME/.bcartifacts.cache/sandbox/$V/w1
BAK=${BAK:-$ART/BusinessCentral-W1.bak}
BACPAC=${BACPAC:-$CACHE/cronus-28.1.bacpac}
BASEAPP=${BASEAPP:-$ART/Extensions/Microsoft_Base Application_$V.app}
COMPANY=${COMPANY:-CRONUS}

if [ ! -x "$PORT" ]; then
  (cd "$ROOT" && CARGO_BUILD_JOBS=4 cargo build --profile bcdb-bench -p al-bcdb-cli)
fi
[ -x "$REF" ] || { echo "no reference bcdb at $REF" >&2; exit 1; }
[ -f "$BAK" ] || { echo "no .bak at $BAK" >&2; exit 1; }

load() { cut -d' ' -f1 /proc/loadavg; }

echo "# bcdb benchmark  $(date -Is)"
echo "host:       $(nproc) cores, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
echo "load at start: $(cut -d' ' -f1-3 /proc/loadavg)"
echo "reference:  $("$REF" --version)"
echo "port:       $("$PORT" --version)  [cargo --profile bcdb-bench: opt-level 3, fat LTO, codegen-units 1, panic=abort]"
echo "runs:       $RUNS timed after 1 untimed warm-up; cache: $([ "$COLD" = 1 ] && echo cold || echo warm)"
echo "files:      $BAK ($(du -h "$BAK" | cut -f1))"
[ -f "$BACPAC" ] && echo "            $BACPAC ($(du -h "$BACPAC" | cut -f1))"
echo

drop_cache() {
  python3 -I - "$@" <<'PY'
import os, sys
for p in sys.argv[1:]:
    try:
        fd = os.open(p, os.O_RDONLY)
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.close(fd)
    except OSError:
        pass
PY
}

RESULTS=$(mktemp)
trap 'rm -f "$RESULTS"' EXIT

# bench <label> <tool> <shell snippet using $T for the tool>
bench() {
  local label=$1 tool=$2 snippet=$3 T i t0 t1 ns=() l0 l1
  [ "$tool" = ref ] && T=$REF || T=$PORT
  export T
  if [ "$COLD" = 1 ]; then drop_cache "$BAK" "$BACPAC" "$BASEAPP"; fi
  eval "$snippet" >/dev/null 2>&1 || true          # warm-up (and page cache fill)
  l0=$(load)
  for i in $(seq 1 "$RUNS"); do
    if [ "$COLD" = 1 ]; then drop_cache "$BAK" "$BACPAC" "$BASEAPP"; fi
    t0=$(date +%s%N)
    eval "$snippet" >/dev/null 2>&1 || true
    t1=$(date +%s%N)
    ns+=($(( (t1 - t0) / 1000 )))
  done
  l1=$(load)
  local sorted min med
  sorted=$(printf '%s\n' "${ns[@]}" | sort -n)
  min=$(echo "$sorted" | head -1)
  med=$(echo "$sorted" | awk '{a[NR]=$1} END{print (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}')
  printf '%s\t%s\t%d\t%d\t%s\n' "$label" "$tool" "$min" "${med%.*}" "load $l0 -> $l1" >>"$RESULTS"
  awk -v l="$label" -v t="$tool" -v a="$min" -v b="${med%.*}" -v ld="load $l0 -> $l1" \
    'BEGIN { printf "  %-34s %-4s min %9.1f ms   median %9.1f ms   (%s)\n", l, t, a/1000, b/1000, ld }'
}

# the requests for a full export: every table with rows in the company, plus tenant-wide ones
make_requests() {
  local file=$1 out=$2
  "$PORT" tables "$file" 2>/dev/null | python3 -I -c '
import sys, json
company = sys.argv[1].upper()
n = 0
for line in sys.stdin:
    head, _, rest = line.rstrip("\n").partition("\t")
    parts = head.split(None, 2)          # rows, compression, company (which may contain spaces)
    rows, comp = int(parts[0]), parts[2].strip()
    name = rest.split("\t")[0]
    if rows == 0: continue
    if comp != "-" and not comp.upper().startswith(company): continue
    req = {"id": n, "cmd": "read", "table": name}
    if comp != "-": req["company"] = comp
    print(json.dumps(req)); n += 1
' "$COMPANY" >"$out"
}

# the largest table of the company by catalog row count, and a small one
pick_tables() {
  local file=$1
  "$PORT" tables "$file" 2>/dev/null | awk -F'\t' -v c="$COMPANY" '
    { n=split($1, h, " "); rows=h[1]; comp=$1; sub(/^ *[0-9]+ +[^ ]+ +/, "", comp); name=$2
      if (toupper(substr(comp,1,length(c))) != toupper(c)) next
      if (rows+0 > best) { best=rows+0; big=name; bigc=comp }
    } END { print big "\t" bigc }'
}

suite() {
  local kind=$1 file=$2
  local big bigc reqs
  IFS=$'\t' read -r big bigc < <(pick_tables "$file")
  reqs=$CACHE/requests-$kind.jsonl
  make_requests "$file" "$reqs"
  echo "## $kind: $(basename "$file")   largest table: \"$big\"   export requests: $(wc -l <"$reqs")"
  export FILE=$file BIG=$big BIGC=$bigc REQS=$reqs APP=$BASEAPP COMPANY
  for tool in ref port; do
    bench "tables (list)"          $tool '"$T" tables "$FILE"'
    bench "describe Customer"      $tool '"$T" describe "$FILE" --table Customer --company "$COMPANY" --symbols "$APP"'
    bench "read small (No. Series)" $tool '"$T" read "$FILE" --table "No. Series" --company "$COMPANY"'
    bench "read large ($big)"      $tool '"$T" read "$FILE" --table "$BIG" --company "$BIGC"'
    bench "full export (serve, all)" $tool '"$T" serve "$FILE" <"$REQS" >/dev/null'
  done
  # the port's own parallel export (not comparable one-to-one: bcdb has no such command)
  bench "full export (port export -j0)" port '"$T" export "$FILE" --company "$COMPANY"'
  bench "full export (port export -j1)" port '"$T" export "$FILE" --company "$COMPANY" --jobs 1'
  echo
}

suite bak "$BAK"
if [ -f "$BACPAC" ]; then suite bacpac "$BACPAC"; else echo "(no .bacpac at $BACPAC: skipped)"; fi

echo "## summary: port / reference (min of $RUNS runs; below 1.00 means the port is faster)"
awk -F'\t' '
  { key=$1; if ($2=="ref") ref[key]=$3; else port[key]=$3; order[key]=NR }
  END { for (k in ref) if (k in port) printf "  %-36s ref %9.1f ms   port %9.1f ms   ratio %.2f\n", k, ref[k]/1000, port[k]/1000, port[k]/ref[k] }' "$RESULTS" | sort
echo "load at end: $(cut -d' ' -f1-3 /proc/loadavg)"
