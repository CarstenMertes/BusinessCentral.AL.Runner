#!/bin/sh
# Installs al-runner from GitHub releases (Linux x64 and arm64, macOS arm64).
#   install.sh [VERSION]
# VERSION: "edge" (newest build of the release branch), "latest-alpha" (default: the newest numbered
# alpha), or a tag such as v3.0.0-alpha.2.
# Environment: ALRUNNER_DOWNLOAD_BASE (a folder or URL holding the files, for tests), ALRUNNER_INSTALL_DIR (default ~/.local/bin), ALRUNNER_REPO (default aldevtools/BusinessCentral.AL.Runner).
set -eu

repo="${ALRUNNER_REPO:-aldevtools/BusinessCentral.AL.Runner}"
dir="${ALRUNNER_INSTALL_DIR:-$HOME/.local/bin}"
want="${1:-latest-alpha}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  *) echo "error: no al-runner build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

fetch() { # URL -> stdout
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"; else wget -qO- "$1"; fi
}
fetch_to() { # URL FILE
  if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$2" "$1"; else wget -qO "$2" "$1"; fi
}

case "$want" in
  edge) tag=edge; label=edge ;;
  latest-alpha)
    # The releases list is newest first. Take the first tag that is not "edge".
    tag="$(fetch "https://api.github.com/repos/$repo/releases?per_page=30" |
      grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' | grep -v '^edge$' | head -n 1)"
    [ -n "$tag" ] || { echo "error: no numbered alpha release found in $repo" >&2; exit 1; }
    label="${tag#v}" ;;
  v*) tag="$want"; label="${want#v}" ;;
  *) tag="v$want"; label="$want" ;;
esac

base="${ALRUNNER_DOWNLOAD_BASE:-https://github.com/$repo/releases/download/$tag}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fetch_to "$base/SHA256SUMS" "$tmp/SHA256SUMS"
# The binary is al-runner. Releases from before the rename (up to 3.0.0-alpha.2) hold alrunner.
exe=al-runner
if ! grep -q " al-runner-$label-$target.tar.xz\$" "$tmp/SHA256SUMS" &&
  grep -q " alrunner-$label-$target.tar.xz\$" "$tmp/SHA256SUMS"; then
  exe=alrunner
fi
archive="$exe-$label-$target.tar.xz"

echo "Installing $exe $tag for $target"
fetch_to "$base/$archive" "$tmp/$archive"

expected="$(grep " $archive\$" "$tmp/SHA256SUMS" | awk '{print $1}')"
[ -n "$expected" ] || { echo "error: $archive is not in SHA256SUMS" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
else actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"; fi
[ "$expected" = "$actual" ] || { echo "error: checksum mismatch for $archive" >&2; exit 1; }

tar -xJf "$tmp/$archive" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/$exe" "$dir/$exe"
# The binary is unsigned. Remove the macOS quarantine flag so Gatekeeper does not block it.
[ "$(uname -s)" = Darwin ] && xattr -d com.apple.quarantine "$dir/$exe" 2>/dev/null || true

"$dir/$exe" --version
case ":$PATH:" in *":$dir:"*) ;; *) echo "Add $dir to your PATH." ;; esac

if [ "$exe" = al-runner ] && [ -e "$dir/alrunner" ]; then
  echo "note: $dir/alrunner is an older build under the old name. Remove it: rm \"$dir/alrunner\""
fi
# AL Runner v2's dotnet tool is also called al-runner. Whichever comes first on PATH runs.
v2dir="$HOME/.dotnet/tools"
if [ "$exe" = al-runner ] && [ -f "$v2dir/al-runner" ]; then
  echo "note: AL Runner v2 is installed at $v2dir/al-runner, a command of the same name."
  echo "  Remove it with \"dotnet tool uninstall -g MSDyn365BC.AL.Runner\" if you no longer need it."
  first=""
  old_ifs="$IFS"; IFS=:
  for p in $PATH; do
    case "$p" in "$dir"|"$v2dir") first="$p"; break ;; esac
  done
  IFS="$old_ifs"
  if [ "$first" = "$v2dir" ]; then
    echo "  It comes first on your PATH now, so \"al-runner\" starts v2. To keep both, put $dir before $v2dir on PATH."
  fi
fi
