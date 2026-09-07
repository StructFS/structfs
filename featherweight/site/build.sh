#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SITE_DIR/../.." && pwd)"

echo "==> Building the demo block (kv.wasm)..."
cargo build --target wasm32-unknown-unknown --release \
  -p featherweight-guest \
  --manifest-path "$REPO_ROOT/Cargo.toml"

echo "==> Copying the browser host + demo block..."
mkdir -p "$SITE_DIR/src/demo"
cp "$REPO_ROOT/featherweight/host/browser/"*.mjs "$SITE_DIR/src/demo/"
cp "$REPO_ROOT/target/wasm32-unknown-unknown/release/featherweight_guest.wasm" \
  "$SITE_DIR/src/demo/kv.wasm"

echo "==> Installing dependencies..."
pnpm install --frozen-lockfile --dir "$SITE_DIR" 2>/dev/null \
  || pnpm install --dir "$SITE_DIR"

echo "==> Building site with 11ty..."
pnpm --dir "$SITE_DIR" exec eleventy

echo "==> Done. Output in $SITE_DIR/_site/"
