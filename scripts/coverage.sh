#!/usr/bin/env bash
set -euo pipefail

# Coverage script for StructFS
# Uses cargo-llvm-cov to generate source-based code coverage reports

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
COVERAGE_DIR="$PROJECT_ROOT/target/coverage"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Generate test coverage reports for StructFS.

Options:
    -h, --help      Show this help message
    --html          Generate HTML report and open in browser (default)
    --lcov          Generate LCOV report only
    --json          Generate JSON report only
    --summary       Show summary in terminal only
    --no-open       Generate HTML but don't open browser
    --clean         Clean coverage data before running

Examples:
    $(basename "$0")              # Generate HTML report and open browser
    $(basename "$0") --summary    # Show coverage summary in terminal
    $(basename "$0") --lcov       # Generate LCOV file for CI integration
EOF
}

# Check if cargo-llvm-cov is installed
check_llvm_cov() {
    if ! command -v cargo-llvm-cov &> /dev/null; then
        echo -e "${YELLOW}cargo-llvm-cov not found. Installing...${NC}"
        cargo install cargo-llvm-cov
        echo -e "${GREEN}cargo-llvm-cov installed successfully${NC}"
    fi
}

# Clean coverage data
clean_coverage() {
    echo "Cleaning coverage data..."
    cargo llvm-cov clean --workspace
    rm -rf "$COVERAGE_DIR"
}

# Generate HTML report
generate_html() {
    local open_browser="$1"

    mkdir -p "$COVERAGE_DIR"

    echo "Running tests with coverage instrumentation..."
    cargo llvm-cov --workspace --html --output-dir "$COVERAGE_DIR"

    echo -e "${GREEN}HTML report generated at: $COVERAGE_DIR/html/index.html${NC}"

    if [[ "$open_browser" == "true" ]]; then
        echo "Opening report in browser..."
        if [[ "$OSTYPE" == "darwin"* ]]; then
            open "$COVERAGE_DIR/html/index.html"
        elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
            xdg-open "$COVERAGE_DIR/html/index.html" 2>/dev/null || \
                sensible-browser "$COVERAGE_DIR/html/index.html" 2>/dev/null || \
                echo "Please open $COVERAGE_DIR/html/index.html in your browser"
        else
            echo "Please open $COVERAGE_DIR/html/index.html in your browser"
        fi
    fi
}

# Generate LCOV report
generate_lcov() {
    mkdir -p "$COVERAGE_DIR"

    echo "Running tests with coverage instrumentation..."
    cargo llvm-cov --workspace --lcov --output-path "$COVERAGE_DIR/lcov.info"

    echo -e "${GREEN}LCOV report generated at: $COVERAGE_DIR/lcov.info${NC}"
}

# Generate JSON report
generate_json() {
    mkdir -p "$COVERAGE_DIR"

    echo "Running tests with coverage instrumentation..."
    cargo llvm-cov --workspace --json --output-path "$COVERAGE_DIR/coverage.json"

    echo -e "${GREEN}JSON report generated at: $COVERAGE_DIR/coverage.json${NC}"
}

# Show summary only
show_summary() {
    echo "Running tests with coverage instrumentation..."
    cargo llvm-cov --workspace
}

# Main
main() {
    cd "$PROJECT_ROOT"

    local mode="html"
    local open_browser="true"
    local do_clean="false"

    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                usage
                exit 0
                ;;
            --html)
                mode="html"
                shift
                ;;
            --lcov)
                mode="lcov"
                shift
                ;;
            --json)
                mode="json"
                shift
                ;;
            --summary)
                mode="summary"
                shift
                ;;
            --no-open)
                open_browser="false"
                shift
                ;;
            --clean)
                do_clean="true"
                shift
                ;;
            *)
                echo -e "${RED}Unknown option: $1${NC}"
                usage
                exit 1
                ;;
        esac
    done

    check_llvm_cov

    if [[ "$do_clean" == "true" ]]; then
        clean_coverage
    fi

    case $mode in
        html)
            generate_html "$open_browser"
            ;;
        lcov)
            generate_lcov
            ;;
        json)
            generate_json
            ;;
        summary)
            show_summary
            ;;
    esac
}

main "$@"
