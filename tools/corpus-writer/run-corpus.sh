#!/usr/bin/env bash
# usage: [SYMS=BC27PLUS,BC28PLUS,BC29PLUS] run-corpus.sh "<codeunit range or a|b|c list>"
# compiles the corpus app, publishes it to the local bc-linux container and runs the given test codeunits; prints failures and a summary.
# Environment: CORPUS (corpus checkout), BCLINUX (bc-linux checkout), WORK (scratch dir), AL_COMPILE (default: al-compile --analyzers none -q)
CORPUS=${CORPUS:-$HOME/corpus}
BCLINUX=${BCLINUX:-$HOME/bc-linux}
W=${WORK:-$HOME/.cache/corpus-writer}
# Compiler: the al-compile wrapper (StefanMaron/al-smart-compile) if it is installed, else the plain AL compiler from bc-linux/scripts/install-al-compiler.sh
# with the symbols staged in $C/.alpackages (see docs/agents/corpus-writer.md).
C=$CORPUS/tests/al-language
mkdir -p $W
rm -f $W/cout.xml
N=$(cat $W/ver.n 2>/dev/null || echo 100); N=$((N+1)); echo $N > $W/ver.n
# every run publishes a NEW app version (a lower or equal version is silently ignored by the container)
sed -i "0,/\"version\": \"1\.0\.[0-9]*\.[0-9]*\"/s//\"version\": \"1.0.0.$N\"/" $C/app.json
if [ -n "$SYMS" ]; then python3 - "$C/app.json" "$SYMS" <<'PY'
import json,sys
p,syms=sys.argv[1],sys.argv[2].split(",")
d=json.load(open(p)); d["preprocessorSymbols"]=syms; json.dump(d,open(p,"w"),indent=2)
PY
fi
APP="$C/AL Language_AL Language Coverage Tests_1.0.0.$N.app"
if [ -n "$AL_COMPILE" ]; then (cd $C && $AL_COMPILE 2>&1 | tail -20)
elif command -v al-compile >/dev/null; then (cd $C && al-compile --analyzers none -q 2>&1 | tail -20)
else AL compile /project:$C /packagecachepath:$C/.alpackages /out:"$APP" ${SYMS:+/define:$SYMS} 2>&1 | grep -E "error|warning AL0(1|2)" | head -20; fi
cd $BCLINUX && ./scripts/run-tests.sh --app "$APP" --codeunit-range "$1" --junit-output $W/cout.xml --timeout 10 > $W/crun.log 2>&1
tail -2 $W/crun.log | head -1; grep -o '"Message":"[^"]*' $W/crun.log | head -1 | cut -c1-500
python3 - "$W" <<'P'
import sys, xml.etree.ElementTree as ET
W = sys.argv[1]
try: t = ET.parse(W + "/cout.xml")
except Exception as e: print("NO RESULT", e); raise SystemExit
p = f = 0
for tc in t.iter('testcase'):
    x = tc.find('failure')
    if x is None: x = tc.find('error')
    if x is None: p += 1; continue
    f += 1
    print("== FAIL", tc.get('classname'), tc.get('name'))
    print((x.text or x.get('message') or '').split('\n\nCall stack')[0][:6000])
print("passed", p, "failed", f)
P
# put back what the build changed (version number, generated layouts, compile log) so the working tree stays clean
cd $CORPUS && git checkout -- tests/al-language/app.json tests/al-language/.dev 2>/dev/null; git clean -fq -- 'tests/al-language/*.app' 2>/dev/null; true
