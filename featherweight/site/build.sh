#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SITE_DIR/../.." && pwd)"
HOST_DIR="$REPO_ROOT/featherweight/host/browser"

echo "==> Building the demo block (kv.wasm)..."
cargo build --target wasm32-unknown-unknown --release \
  -p featherweight-guest --features reference-guest \
  --manifest-path "$REPO_ROOT/Cargo.toml"

# The browser host is TypeScript; its own `build` script (tsc) emits ES
# modules to dist/ with relative imports rewritten to `.js`.
echo "==> Compiling the browser host..."
npm ci --prefix "$HOST_DIR" --no-audit --no-fund
npm run --prefix "$HOST_DIR" build

echo "==> Copying the browser host + demo block..."
rm -rf "$SITE_DIR/src/demo"
mkdir -p "$SITE_DIR/src/demo"
cp "$HOST_DIR/dist/"*.js "$SITE_DIR/src/demo/"
cp "$REPO_ROOT/target/wasm32-unknown-unknown/release/featherweight_guest.wasm" \
  "$SITE_DIR/src/demo/kv.wasm"

echo "==> Installing dependencies..."
pnpm install --frozen-lockfile --dir "$SITE_DIR" 2>/dev/null \
  || pnpm install --dir "$SITE_DIR"

echo "==> Building site with 11ty..."
pnpm --dir "$SITE_DIR" exec eleventy

echo "==> Done. Output in $SITE_DIR/_site/"
