#!/usr/bin/env bash
# Populate caches for every independent Rust graph and the browser host.
# This step may access the network. Verification remains locked and offline.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fetch --locked
for consumer in tests/embedding tests/portable tests/applications/*; do
    cargo fetch --locked --manifest-path "$consumer/Cargo.toml"
done
npm ci --prefix featherweight/host/browser --no-audit --no-fund
