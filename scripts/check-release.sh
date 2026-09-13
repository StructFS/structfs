#!/usr/bin/env bash
# The same candidate checks run locally and on every release CI host.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 -m unittest discover -s scripts -p 'test_release.py'
cargo fmt --all -- --check
for consumer in tests/embedding tests/portable tests/applications/*; do
    cargo fmt --manifest-path "$consumer/Cargo.toml" -- --check
done
cargo test --workspace --all-features --locked --offline
cargo clippy --workspace --all-features --all-targets --locked --offline -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked --offline
# Avoid workspace feature unification hiding missing feature dependencies.
for feature in '' serde json http sys async service state profiles full; do
    if [[ -z "$feature" ]]; then
        cargo check -p structfs --no-default-features --locked --offline
    else
        cargo check -p structfs --no-default-features --features "$feature" --locked --offline
    fi
done
# Standalone graphs catch native features leaking into browser-shared code.
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --target wasm32-unknown-unknown
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --features native
scripts/check-featherweight-release.sh
