#!/usr/bin/env bash
set -euo pipefail

printf "fmt: "
if cargo fmt --all -- --check >/dev/null 2>&1; then
    echo "ok"
else
    echo "reformatted"
    cargo fmt --all --quiet
fi

printf "clippy: "
cargo clippy --workspace --all-targets --quiet -- -D warnings && echo "ok"

printf "test: "
cargo test --workspace --quiet && echo "ok"
