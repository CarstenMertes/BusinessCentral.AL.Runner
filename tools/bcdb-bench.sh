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
export LC_ALL=C   # decimal points in the time and awk output

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
# Records wall time and CPU time (user+sys of the process and its children). CPU time moves
# far less than wall time when other jobs share the machine, and it is what tells apart a
# slow program from a loaded host; but a program that uses several threads (bcdb's prefetch,
# the port's export) spends more CPU than wall, so read the two together.
bench() {
  local label="[$KIND] $1" tool=$2 snippet=$3 T i t0 t1 wall=() cpu=() l0 l1 c
  [ "$tool" = ref ] && T=$REF || T=$PORT
  export T
  if [ "$COLD" = 1 ]; then drop_cache "$BAK" "$BACPAC" "$BASEAPP"; fi
  eval "$snippet" >/dev/null 2>&1 || true          # warm-up (and page cache fill)
  l0=$(load)
  for i in $(seq 1 "$RUNS"); do
    if [ "$COLD" = 1 ]; then drop_cache "$BAK" "$BACPAC" "$BASEAPP"; fi
    t0=$(date +%s%N)
    c=$( { TIMEFORMAT='%3U %3S'; time eval "$snippet" >/dev/null 2>&1 || true; } 2>&1 )
    t1=$(date +%s%N)
    wall+=($(( (t1 - t0) / 1000 )))
    cpu+=($(echo "$c" | awk '{printf "%d", ($1 + $2) * 1000000}'))
  done
  l1=$(load)
  stat() {  # min and median of the numbers on stdin
    sort -n | awk '{a[NR]=$1} END{printf "%d %d", a[1], (NR%2)?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2}'
  }
  local w c2
  w=$(printf '%s\n' "${wall[@]}" | stat)
  c2=$(printf '%s\n' "${cpu[@]}" | stat)
  printf '%s\t%s\t%s\t%s\t%s\n' "$label" "$tool" "$w" "$c2" "load $l0 -> $l1" >>"$RESULTS"
  awk -v l="$label" -v t="$tool" -v w="$w" -v c="$c2" -v ld="load $l0 -> $l1" 'BEGIN {
    split(w, a, " "); split(c, b, " ")
    printf "  %-34s %-4s wall min %9.1f med %9.1f ms | cpu min %9.1f med %9.1f ms  (%s)\n", l, t, a[1]/1000, a[2]/1000, b[1]/1000, b[2]/1000, ld }'
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
  KIND=$kind
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

echo "## summary: port / reference, min of $RUNS runs (below 1.00 the port is faster)"
awk -F'\t' '
  { split($3, w, " "); split($4, c, " ")
    if ($2 == "ref") { rw[$1] = w[1]; rc[$1] = c[1] } else { pw[$1] = w[1]; pc[$1] = c[1] } }
  END { for (k in rw) if (k in pw)
          printf "  %-36s wall %9.1f -> %9.1f ms  x%.2f   cpu %9.1f -> %9.1f ms  x%.2f\n", k, rw[k]/1000, pw[k]/1000, pw[k]/rw[k], rc[k]/1000, pc[k]/rc[k] }' "$RESULTS" | sort
echo "load at end: $(cut -d' ' -f1-3 /proc/loadavg)"
