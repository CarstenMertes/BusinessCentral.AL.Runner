#!/usr/bin/env bash
# Smoke test of the packed .NET tool: installs MSDyn365BC.AL.Runner VERSION from the .nupkg in
# NUPKG_DIR (and nowhere else) into a new tool folder, checks that al-runner --version prints VERSION,
# then runs release/smoke.sh through the tool.
# Usage: packaging/dotnet-tool/smoke.sh NUPKG_DIR VERSION
set -euo pipefail
nupkgs="$(cd "$1" && pwd)"
version="$2"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
# A source path in nuget.config is read relative to the file, which keeps Windows paths out of it.
cp -r "$nupkgs" "$work/feed"
cat > "$work/feed/nuget.config" <<'CFG'
<?xml version="1.0" encoding="utf-8"?>
<configuration><packageSources><clear /><add key="local" value="." /></packageSources></configuration>
CFG
export NUGET_PACKAGES="$work/packages"
(cd "$work" && dotnet tool install MSDyn365BC.AL.Runner --version "$version" --tool-path tools --configfile feed/nuget.config)
tool="$work/tools/al-runner"
[ -f "$tool.exe" ] && tool="$tool.exe"
printed="$("$tool" --version | tr -d '\r' | head -n 1)"
echo "$printed"
case "$printed" in
  "al-runner $version "*|"al-runner $version") ;;
  *) echo "error: the tool prints '$printed', not version $version" >&2; exit 1 ;;
esac
"$here/../../release/smoke.sh" "$tool"
echo "the .NET tool smoke test passed"
