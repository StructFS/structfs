#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Building WASM..."
wasm-pack build --target web \
  --out-dir "$SITE_DIR/src/wasm/pkg" \
  "$SITE_DIR/wasm"

echo "==> Installing dependencies..."
pnpm install --frozen-lockfile --dir "$SITE_DIR" 2>/dev/null \
  || pnpm install --dir "$SITE_DIR"

echo "==> Building site with 11ty..."
pnpm --dir "$SITE_DIR" exec eleventy

echo "==> Done. Output in $SITE_DIR/_site/"
