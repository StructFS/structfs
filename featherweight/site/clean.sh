#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Cleaning build artifacts..."

rm -rf "$SITE_DIR/_site"
echo "    removed _site/"

rm -rf "$SITE_DIR/src/demo"
echo "    removed src/demo/ (copied)"

rm -rf "$SITE_DIR/node_modules"
echo "    removed node_modules/"

echo "==> Clean."
