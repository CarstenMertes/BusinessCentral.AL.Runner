#!/usr/bin/env bash
# Regenerates docs/language-catalog.json by asking the AL language server of each BC major (27, 28,
# 29) about probe files, the way VS Code uses it (lsp_catalog.py). Nothing of Microsoft's is
# written to the repository: only names and signatures from the server's answers.
#
# The AL Language extension of major M comes from, in order:
#   1. AL_EXTENSION_<M>: an extension folder (with bin/ and package.json);
#   2. the ALLanguage.vsix in the BC artifacts cache (~/.bcartifacts.cache/sandbox/M.*), extracted
#      to ~/.cache/al-runtime/language-catalog/<M> (never into the repository);
#   3. an installed VS Code AL extension of runtime M-11 (~/.vscode/extensions/ms-dynamics-smb.al-<M-11>.*).
# The platform symbols (System.app) come from the same artifacts folder when it has them.
#
# Usage: tools/language-catalog/generate.sh [--check]   (--check: fail if the committed file differs)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cache="${HOME}/.cache/al-runtime/language-catalog"
mkdir -p "$cache"
specs=()
for m in 27 28 29; do
  var="AL_EXTENSION_${m}"
  ext="${!var:-}"
  sysapp=""
  vsix="$(find "${HOME}/.bcartifacts.cache/sandbox" -maxdepth 8 -ipath "*/sandbox/${m}.*" -iname ALLanguage.vsix 2>/dev/null | sort | tail -1)"
  sysapp="$(find "${HOME}/.bcartifacts.cache/sandbox" -maxdepth 8 -ipath "*/sandbox/${m}.*" -ipath '*AL Development Environment*' -iname System.app 2>/dev/null | sort | tail -1)"
  if [[ -z "$ext" && -n "$vsix" ]]; then
    ext="${cache}/${m}/extension"
    if [[ ! -e "${ext}/bin/linux/Microsoft.Dynamics.Nav.EditorServices.Host" ]]; then
      mkdir -p "${cache}/${m}"
      unzip -q -o "$vsix" 'extension/bin/linux/*' 'extension/package.json' -d "${cache}/${m}"
    fi
    chmod +x "${ext}/bin/linux/Microsoft.Dynamics.Nav.EditorServices.Host"
  fi
  if [[ -z "$ext" ]]; then
    ext="$(ls -d "${HOME}"/.vscode/extensions/ms-dynamics-smb.al-$((m - 11)).* 2>/dev/null | sort -V | tail -1 || true)"
  fi
  if [[ -x "${ext}/bin/linux/Microsoft.Dynamics.Nav.EditorServices.Host" ]]; then
    host="${ext}/bin/linux/Microsoft.Dynamics.Nav.EditorServices.Host"
  elif [[ -f "${ext}/bin/Microsoft.Dynamics.Nav.EditorServices.Host.dll" ]]; then
    host="dotnet"$'\t'"${ext}/bin/Microsoft.Dynamics.Nav.EditorServices.Host.dll"
  else
    echo "no AL Language extension found for BC ${m}; set AL_EXTENSION_${m}" >&2
    exit 2
  fi
  version="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "${ext}/package.json")"
  specs+=("${m}=${host}|${sysapp}|${version}")
done
out="${root}/docs/language-catalog.json"
[[ "${1:-}" == "--check" ]] && out="$(mktemp -p "${cache}" check.XXXXXX.json)"
python3 -I "${root}/tools/language-catalog/lsp_catalog.py" --out "$out" --work "$cache" "${specs[@]}"
if [[ "${1:-}" == "--check" ]]; then
  if ! diff -q "$out" "${root}/docs/language-catalog.json" >/dev/null; then
    echo "docs/language-catalog.json is stale: run tools/language-catalog/generate.sh" >&2
    rm -f "$out"; exit 1
  fi
  rm -f "$out"; echo "docs/language-catalog.json is current"
fi
