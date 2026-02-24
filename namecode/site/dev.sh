#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NAMECODE_DIR="$(cd "$SITE_DIR/../.." && pwd)/namecode"

cleanup() {
  echo ""
  echo "==> Shutting down..."
  kill 0 2>/dev/null
  wait 2>/dev/null
}
trap cleanup EXIT INT TERM

echo "==> Installing dependencies..."
pnpm install --frozen-lockfile --dir "$SITE_DIR" 2>/dev/null \
  || pnpm install --dir "$SITE_DIR"

echo "==> Building WASM..."
wasm-pack build --target web \
  --out-dir "$SITE_DIR/src/wasm/pkg" \
  "$SITE_DIR/wasm"

# Watch Rust sources and rebuild WASM on change
echo "==> Starting WASM watcher..."
if command -v cargo-watch &>/dev/null; then
  cargo watch \
    -w "$SITE_DIR/wasm/src" \
    -w "$NAMECODE_DIR/src" \
    -s "wasm-pack build --target web --out-dir \"$SITE_DIR/src/wasm/pkg\" \"$SITE_DIR/wasm\"" \
    &
elif command -v fswatch &>/dev/null; then
  (
    fswatch -o "$SITE_DIR/wasm/src" "$NAMECODE_DIR/src" | while read -r; do
      echo "==> Rust source changed, rebuilding WASM..."
      wasm-pack build --target web \
        --out-dir "$SITE_DIR/src/wasm/pkg" \
        "$SITE_DIR/wasm" || true
    done
  ) &
else
  echo "    (install cargo-watch or fswatch for auto WASM rebuilds)"
fi

echo "==> Starting dev server (http://localhost:8080)..."
pnpm --dir "$SITE_DIR" exec eleventy --serve --watch
