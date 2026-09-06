#!/usr/bin/env bash
# Build the reference guest and run the JS host's Node tests.
#
# Needs the wasm32-unknown-unknown target (rustup target add
# wasm32-unknown-unknown) and Node 20+. Not part of quality_gates.sh:
# those stay hermetic to the Rust toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p featherweight-guest --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/featherweight_guest.wasm \
   featherweight/host/browser/kv.wasm

node --test featherweight/host/browser/test/host.test.mjs
