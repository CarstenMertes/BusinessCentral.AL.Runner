#!/usr/bin/env bash
# Smoke test of a built al-runner binary: --version, then the fixture project in release/smoke.
# Usage: release/smoke.sh PATH_TO_BINARY
# No Microsoft code, no dependency download, no test data. Exit 0 only when both tests pass.
set -euo pipefail
bin="$1"
here="$(cd "$(dirname "$0")" && pwd)"
version="$("$bin" --version | tr -d '\r')"
echo "$version" >&2
echo "$version" | grep -Eq '^al-runner [0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?(\+[0-9A-Za-z.]+)? \(([0-9a-f]+|unknown)\)$'
out="$("$bin" test "$here/smoke/App" "$here/smoke/Test" --no-download --no-test-data --verbose 2>&1)" || { echo "$out"; echo "smoke test failed" >&2; exit 1; }
echo "$out"
echo "$out" | grep -q 'AddAddsTwoNumbers'
