#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
status=0
cargo fmt --all --check || status=1
cargo clippy --workspace --all-targets -- -D warnings || status=1
cargo test --workspace -- --test-threads=2 || status=1
exit "$status"
