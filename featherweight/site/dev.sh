#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pick a random port between 8100-8999
PORT=$((RANDOM % 900 + 8100))

# Refresh the copied inputs (browser host, kv.wasm), install, then
# serve. Note: the eleventy dev server does NOT send the COOP/COEP
# headers, so the resident demo runs in batch-degraded mode locally;
# use `wrangler pages dev _site` to test cross-origin isolation.
"$SITE_DIR/build.sh"

echo "==> Starting dev server (http://localhost:$PORT)..."
pnpm --dir "$SITE_DIR" exec eleventy --serve --watch --port="$PORT"
