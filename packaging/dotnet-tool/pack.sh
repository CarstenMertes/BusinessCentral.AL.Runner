#!/usr/bin/env bash
# Builds the .NET tool package MSDyn365BC.AL.Runner from the release archives.
# Usage: packaging/dotnet-tool/pack.sh VERSION DIST_DIR OUT_DIR
#   VERSION   the package version, for example 3.0.0-alpha.6 (the archives carry it in their names)
#   DIST_DIR  the folder with al-runner-VERSION-<target>.tar.xz / .zip (the "packages" artifact)
#   OUT_DIR   where MSDyn365BC.AL.Runner.VERSION.nupkg is written
# Unpacks each binary into native/<rid>/ next to this script, then runs dotnet pack.
set -euo pipefail
version="$1"
dist="$(cd "$2" && pwd)"
mkdir -p "$3"
out="$(cd "$3" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"

rm -rf "$here/native" "$here/bin" "$here/obj"
# rid  release target  archive extension  binary name
while read -r rid target ext exe; do
  archive="$dist/al-runner-$version-$target.$ext"
  [ -f "$archive" ] || { echo "error: missing $archive" >&2; exit 1; }
  mkdir -p "$here/native/$rid"
  if [ "$ext" = zip ]; then
    unzip -q -o -j "$archive" "$exe" -d "$here/native/$rid"
  else
    tar -C "$here/native/$rid" -xJf "$archive" "./$exe"
  fi
  [ -f "$here/native/$rid/$exe" ] || { echo "error: $archive holds no $exe" >&2; exit 1; }
done <<'TARGETS'
linux-x64 x86_64-unknown-linux-musl tar.xz al-runner
osx-arm64 aarch64-apple-darwin tar.xz al-runner
win-x64 x86_64-pc-windows-msvc zip al-runner.exe
TARGETS

dotnet pack "$here/AlRunner.Tool.csproj" -c Release -p:Version="$version" -o "$out" --nologo
ls -l "$out"/*.nupkg
