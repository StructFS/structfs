#!/usr/bin/env bash
set -euo pipefail

# Build and run a WASM Block through the Featherweight runtime.
#
# Usage:
#   ./scripts/run_wasm_block.sh              # Build and run the default hello block
#   ./scripts/run_wasm_block.sh path/to.wasm # Run a pre-built component

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GUEST_CRATE="featherweight-guest"
TARGET="wasm32-unknown-unknown"
PROFILE="release"

WASM_OUTPUT="$PROJECT_ROOT/target/$TARGET/$PROFILE/featherweight_guest.wasm"
COMPONENT_OUTPUT="$PROJECT_ROOT/target/$TARGET/$PROFILE/featherweight_guest_component.wasm"

build_guest() {
    echo "Building guest crate for WASM..."
    cargo build -p "$GUEST_CRATE" --target "$TARGET" --release --quiet
    echo "  Built: $WASM_OUTPUT"
}

convert_to_component() {
    echo "Converting to WASM component..."
    if ! command -v wasm-tools &> /dev/null; then
        echo "Error: wasm-tools not found. Install with: cargo install wasm-tools"
        exit 1
    fi
    wasm-tools component new "$WASM_OUTPUT" -o "$COMPONENT_OUTPUT"
    echo "  Created: $COMPONENT_OUTPUT"
}

run_block() {
    local component="$1"
    echo ""
    echo "Running WASM Block..."
    echo "----------------------------------------"
    cargo run --example wasm_hello -p featherweight-runtime --quiet -- "$component"
}

main() {
    cd "$PROJECT_ROOT"

    if [[ $# -gt 0 ]]; then
        # Run with provided component
        run_block "$1"
    else
        # Build and run default
        build_guest
        convert_to_component
        run_block "$COMPONENT_OUTPUT"
    fi
}

main "$@"
