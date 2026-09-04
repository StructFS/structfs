#!/usr/bin/env bash
set -euo pipefail

# Build the wasm-kv guest Block (core-wasm binding, spec 11) and run it
# inside a Featherweight assembly behind the interactive shell.
#
# Usage:
#   ./scripts/run_wasm_block.sh              # Build the guest and run the demo
#   ./scripts/run_wasm_block.sh path/to.wasm # Run a pre-built block
#
# The core binding needs no componentization: plain
# `cargo build --target wasm32-unknown-unknown` output runs directly.
# (Component-model blocks also work — the runtime sniffs the artifact.)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TARGET="wasm32-unknown-unknown"
WASM_OUTPUT="$PROJECT_ROOT/target/$TARGET/release/featherweight_guest.wasm"

if [[ $# -ge 1 ]]; then
    WASM_OUTPUT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
else
    echo "Building guest crate for wasm (core binding)..."
    cargo build -p featherweight-guest --target "$TARGET" --release --quiet
fi

# An assembly with the shell in front of the wasm kv block.
ASSEMBLY_DIR="$(mktemp -d)"
trap 'rm -rf "$ASSEMBLY_DIR"' EXIT
cp "$WASM_OUTPUT" "$ASSEMBLY_DIR/wasm_kv.wasm"
cat >"$ASSEMBLY_DIR/wasm_demo.assembly.yaml" <<'EOF'
assembly: wasm-demo
blocks:
  shell:
    artifact: builtin:shell
    stdio: host
  kv: wasm_kv.wasm
public: shell
wiring:
  - "shell:/services/kv -> kv"
config:
  shell:
    prompt: "wasm> "
EOF

echo ""
echo "Running: try 'write services/kv/greeting \"hello\"' then 'read services/kv/greeting'"
echo "----------------------------------------"
exec cargo run -q -p featherweight -- run "$ASSEMBLY_DIR/wasm_demo.assembly.yaml"
