#!/usr/bin/env bash
set -euo pipefail

# Build the wasm-kv guest Block, componentize it, and run it inside a
# Featherweight assembly behind the interactive shell.
#
# Usage:
#   ./scripts/run_wasm_block.sh              # Build the guest and run the demo
#   ./scripts/run_wasm_block.sh path/to.wasm # Run a pre-built component
#
# Requires: the wasm32-unknown-unknown target and wasm-tools
# (cargo install wasm-tools).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TARGET="wasm32-unknown-unknown"
WASM_OUTPUT="$PROJECT_ROOT/target/$TARGET/release/featherweight_guest.wasm"
COMPONENT_OUTPUT="$PROJECT_ROOT/target/$TARGET/release/wasm_kv_component.wasm"

if [[ $# -ge 1 ]]; then
    COMPONENT_OUTPUT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
else
    echo "Building guest crate for wasm..."
    cargo build -p featherweight-guest --target "$TARGET" --release --quiet

    if ! command -v wasm-tools &>/dev/null; then
        echo "Error: wasm-tools not found. Install with: cargo install wasm-tools" >&2
        exit 1
    fi
    echo "Converting to a wasm component..."
    wasm-tools component new "$WASM_OUTPUT" -o "$COMPONENT_OUTPUT"
fi

# An assembly with the shell in front of the wasm kv block.
ASSEMBLY_DIR="$(mktemp -d)"
trap 'rm -rf "$ASSEMBLY_DIR"' EXIT
cp "$COMPONENT_OUTPUT" "$ASSEMBLY_DIR/wasm_kv.wasm"
cat >"$ASSEMBLY_DIR/wasm_demo.assembly.yaml" <<'EOF'
assembly: wasm-demo
blocks:
  shell: builtin:shell
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
