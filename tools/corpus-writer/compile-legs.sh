#!/usr/bin/env bash
# Compile the corpus app with the AL compiler and preprocessor symbols of each CI leg group, before every push.
# CI uses a different compiler per BC major (27: 16.x, 28: 17.x, 29: 18.x) and some code compiles on one and not on another
# (for example a FlowField of type Integer over a BigInteger field: AL0427 on 16.x and 17.x only).
# usage: CORPUS=~/corpus BCLINUX=~/bc-linux tools/corpus-writer/compile-legs.sh
# Needs the symbols in $CORPUS/tests/al-language/.alpackages (the same ones the 29 compile uses). Exit 1 when any leg has an error.
set -uo pipefail
CORPUS=${CORPUS:?set CORPUS to the corpus clone}
BCLINUX=${BCLINUX:?set BCLINUX to the bc-linux clone}
APP=$CORPUS/tests/al-language
cp "$APP/app.json" "$APP/app.json.legs-bak"
restore() { mv -f "$APP/app.json.legs-bak" "$APP/app.json"; (cd "$CORPUS" && { git diff --name-only -- tests/al-language | grep -E '\.(rdl|rdlc|docx|xlsx|app|log)$' | xargs -r git checkout -q --; git ls-files --others --exclude-standard -- tests/al-language | grep -E '\.(rdl|rdlc|docx|xlsx)$' | xargs -r rm -f; }); }
trap restore EXIT
status=0
for leg in "27.0 BC27PLUS" "28.0 BC27PLUS,BC28PLUS" "29.0 BC27PLUS,BC28PLUS,BC29PLUS"; do
  set -- $leg
  dir=$(bash "$BCLINUX/scripts/install-al-compiler.sh" "$1" 2>/dev/null | sed -n 's/^AL_TOOL_DIR=//p')
  python3 - "$APP/app.json" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); d["preprocessorSymbols"] = sys.argv[2].split(",")
json.dump(d, open(sys.argv[1], "w"), indent=2)
PY
  if [ -x "$dir/alc" ]; then alc=("$dir/alc"); else alc=(dotnet "$dir/alc.dll"); fi
  out=$(cd "$APP" && "${alc[@]}" /project:. /packagecachepath:.alpackages /out:"${TMPDIR:-/tmp}/legs-$1.app" 2>&1)
  errs=$(printf '%s\n' "$out" | grep -E ': error ' || true)
  if [ -n "$errs" ]; then echo "== BC $1 ($2): ERRORS"; printf '%s\n' "$errs" | head -20; status=1; else echo "== BC $1 ($2): ok"; fi
done
exit $status
