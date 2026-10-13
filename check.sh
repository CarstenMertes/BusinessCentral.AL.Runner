#!/usr/bin/env bash
# The pre-commit gate: the same checks CI runs, failing on the first error.
set -euo pipefail
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace -q
