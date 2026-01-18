#!/usr/bin/env bash
set -euo pipefail

# Coverage script for StructFS
# Uses cargo-llvm-cov to generate source-based code coverage reports
#
# IMPORTANT: If you modify this script, update the manual at:
#   docs/coverage.md

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
COVERAGE_DIR="$PROJECT_ROOT/target/coverage"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m' # No Color

# Default threshold for "needs coverage"
DEFAULT_THRESHOLD=95

usage() {
    cat <<EOF
Usage: $(basename "$0") [COMMAND] [OPTIONS]

Analyze and generate test coverage reports for StructFS.

Commands:
    (none)          Show summary with overall coverage and top gaps
    gaps            List files below threshold, sorted by uncovered lines
    file <path>     Show uncovered lines for a specific file
    html            Generate HTML report and open in browser
    lcov            Generate LCOV report for CI integration
    json            Generate JSON report

Options:
    -h, --help          Show this help message
    -t, --threshold N   Coverage threshold percentage (default: $DEFAULT_THRESHOLD)
    -n, --top N         Show top N files (default: 10 for summary, all for gaps)
    --no-open           Don't open browser for HTML report
    --clean             Clean coverage data before running
    --skip-tests        Use cached coverage data (skip running tests)

Examples:
    $(basename "$0")                    # Summary with top 10 gaps
    $(basename "$0") gaps               # All files below ${DEFAULT_THRESHOLD}%
    $(basename "$0") gaps -t 90         # All files below 90%
    $(basename "$0") file src/foo.rs    # Uncovered lines in foo.rs
    $(basename "$0") html               # Open detailed HTML report
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

# Run tests and cache coverage output
ensure_coverage_data() {
    local skip_tests="$1"
    local summary_file="$COVERAGE_DIR/coverage_summary.txt"
    local detail_file="$COVERAGE_DIR/coverage_detail.txt"

    if [[ "$skip_tests" == "true" && -f "$summary_file" && -f "$detail_file" ]]; then
        echo -e "${DIM}Using cached coverage data${NC}" >&2
        return 0
    fi

    mkdir -p "$COVERAGE_DIR"
    echo -e "${DIM}Running tests with coverage instrumentation...${NC}" >&2

    # Run tests once to generate profiling data, capture summary output
    cargo llvm-cov --workspace 2>&1 | tee "$summary_file" | grep -E "^(running|test |TOTAL)" >&2 || true

    # Generate detailed line-by-line output without re-running tests
    echo -e "${DIM}Generating detailed coverage report...${NC}" >&2
    cargo llvm-cov report --text 2>&1 > "$detail_file"

    echo -e "${DIM}Coverage data cached.${NC}" >&2
}

# Get cached summary (per-file stats and TOTAL)
get_coverage_summary() {
    cat "$COVERAGE_DIR/coverage_summary.txt"
}

# Get cached detail (line-by-line)
get_coverage_detail() {
    cat "$COVERAGE_DIR/coverage_detail.txt"
}

# Show summary: overall coverage + top gaps
show_summary() {
    local threshold="$1"
    local top_n="$2"
    local skip_tests="$3"

    ensure_coverage_data "$skip_tests"

    echo ""
    # Get the TOTAL line from cached summary
    local summary
    summary=$(get_coverage_summary | grep "^TOTAL")

    # Parse overall coverage from TOTAL line
    # Format: TOTAL  regions_total  regions_missed  regions_pct%  ...
    local total_regions total_missed region_pct
    total_regions=$(echo "$summary" | awk '{print $2}')
    total_missed=$(echo "$summary" | awk '{print $3}')
    region_pct=$(echo "$summary" | awk '{print $4}' | tr -d '%')

    # Color the percentage based on threshold
    local pct_color="$GREEN"
    if (( $(echo "$region_pct < $threshold" | bc -l) )); then
        pct_color="$YELLOW"
    fi
    if (( $(echo "$region_pct < 80" | bc -l) )); then
        pct_color="$RED"
    fi

    echo -e "${BOLD}Overall Coverage:${NC} ${pct_color}${region_pct}%${NC} ${DIM}(target: ${threshold}%)${NC}"
    echo ""

    # Show top gaps
    echo -e "${BOLD}Top Coverage Gaps:${NC}"
    show_gaps_internal "$threshold" "$top_n" "true"
}

# Show files below threshold sorted by uncovered lines
show_gaps() {
    local threshold="$1"
    local top_n="$2"
    local skip_tests="$3"

    ensure_coverage_data "$skip_tests"

    echo ""
    echo -e "${BOLD}Files below ${threshold}% coverage (sorted by uncovered lines):${NC}"
    echo ""
    show_gaps_internal "$threshold" "$top_n" "false"
}

# Internal: parse and display gaps
show_gaps_internal() {
    local threshold="$1"
    local limit="$2"
    local compact="$3"

    # Get summary output and parse file coverage
    # Format: filename  regions_total regions_missed regions_pct% ...
    # We want columns 1 (file), 3 (regions missed), 4 (regions pct)
    get_coverage_summary | \
        grep -E "^(packages/|featherweight/)" | \
        awk -v threshold="$threshold" '
        {
            file = $1
            missed = $3
            pct = $4
            gsub(/%/, "", pct)

            # Skip files at or above threshold
            if (pct + 0 >= threshold + 0) next

            # Output: missed pct file (for sorting)
            printf "%d\t%.2f\t%s\n", missed, pct, file
        }' | \
        sort -t$'\t' -k1 -nr | \
        head -n "$limit" | \
        while IFS=$'\t' read -r missed pct file; do
            # Color based on coverage level
            local pct_color="$YELLOW"
            if (( $(echo "$pct < 80" | bc -l) )); then
                pct_color="$RED"
            fi
            if (( $(echo "$pct < 50" | bc -l) )); then
                pct_color="$RED${BOLD}"
            fi

            if [[ "$compact" == "true" ]]; then
                printf "  ${pct_color}%5.1f%%${NC}  %4d uncovered  %s\n" "$pct" "$missed" "$file"
            else
                printf "${pct_color}%6.2f%%${NC}  %5d uncovered  %s\n" "$pct" "$missed" "$file"
            fi
        done

    echo ""
}

# Show uncovered lines for a specific file
show_file() {
    local file_pattern="$1"
    local skip_tests="$2"

    ensure_coverage_data "$skip_tests"

    # Find matching file in detailed coverage output
    local matching_files
    matching_files=$(get_coverage_detail | grep -E "^$file_pattern:" | head -1 | cut -d: -f1 || true)

    if [[ -z "$matching_files" ]]; then
        # Try partial match
        matching_files=$(get_coverage_detail | grep -E "^[^|]+$file_pattern[^|]*:" | head -1 | cut -d: -f1 || true)
    fi

    if [[ -z "$matching_files" ]]; then
        echo -e "${RED}No coverage data found for: $file_pattern${NC}"
        echo "Try a more specific path or check that the file has tests."
        exit 1
    fi

    echo -e "${BOLD}Uncovered lines in:${NC} $matching_files"
    echo ""

    # Extract the file's coverage and show uncovered lines
    get_coverage_detail | \
        awk -v file="$matching_files" '
            BEGIN { in_file = 0 }
            $0 ~ "^" file ":" { in_file = 1; next }
            /^[a-zA-Z_\/].*:$/ { if (in_file) exit }
            in_file && /^[ \t]+[0-9]+\|[ \t]+0\|/ {
                # Line with 0 coverage - extract line number and content
                # Format: "   123|      0|    code here"
                split($0, parts, "|")
                linenum = parts[1] + 0
                content = parts[3]
                for (i = 4; i <= length(parts); i++) content = content "|" parts[i]
                printf "\033[33m%4d\033[0m │%s\n", linenum, content
            }
        ' || true

    echo ""

    # Show summary for this file
    local file_summary
    file_summary=$(get_coverage_summary | grep -F "$matching_files" | head -1 || true)
    if [[ -n "$file_summary" ]]; then
        local pct missed
        pct=$(echo "$file_summary" | awk '{print $4}')
        missed=$(echo "$file_summary" | awk '{print $3}')
        echo -e "${DIM}Coverage: $pct ($missed lines uncovered)${NC}"
    fi
}

# Generate HTML report
generate_html() {
    local open_browser="$1"
    local skip_tests="$2"

    mkdir -p "$COVERAGE_DIR"

    if [[ "$skip_tests" != "true" ]]; then
        echo "Running tests with coverage instrumentation..."
        cargo llvm-cov --workspace --html --output-dir "$COVERAGE_DIR"
    else
        echo "Generating HTML from cached data..."
        cargo llvm-cov --workspace --html --output-dir "$COVERAGE_DIR" --no-run
    fi

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
    local skip_tests="$1"
    mkdir -p "$COVERAGE_DIR"

    if [[ "$skip_tests" != "true" ]]; then
        echo "Running tests with coverage instrumentation..."
    fi
    cargo llvm-cov --workspace --lcov --output-path "$COVERAGE_DIR/lcov.info"

    echo -e "${GREEN}LCOV report generated at: $COVERAGE_DIR/lcov.info${NC}"
}

# Generate JSON report
generate_json() {
    local skip_tests="$1"
    mkdir -p "$COVERAGE_DIR"

    if [[ "$skip_tests" != "true" ]]; then
        echo "Running tests with coverage instrumentation..."
    fi
    cargo llvm-cov --workspace --json --output-path "$COVERAGE_DIR/coverage.json"

    echo -e "${GREEN}JSON report generated at: $COVERAGE_DIR/coverage.json${NC}"
}

# Main
main() {
    cd "$PROJECT_ROOT"

    local command=""
    local threshold="$DEFAULT_THRESHOLD"
    local top_n="10"
    local open_browser="true"
    local do_clean="false"
    local skip_tests="false"
    local file_arg=""

    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                usage
                exit 0
                ;;
            -t|--threshold)
                threshold="$2"
                shift 2
                ;;
            -n|--top)
                top_n="$2"
                shift 2
                ;;
            --no-open)
                open_browser="false"
                shift
                ;;
            --clean)
                do_clean="true"
                shift
                ;;
            --skip-tests)
                skip_tests="true"
                shift
                ;;
            gaps|html|lcov|json)
                command="$1"
                shift
                ;;
            file)
                command="file"
                if [[ $# -lt 2 ]]; then
                    echo -e "${RED}Error: 'file' command requires a path argument${NC}"
                    exit 1
                fi
                file_arg="$2"
                shift 2
                ;;
            *)
                # Check if it looks like a file path for the default file command
                if [[ "$1" == *"/"* || "$1" == *".rs" ]]; then
                    command="file"
                    file_arg="$1"
                    shift
                else
                    echo -e "${RED}Unknown option: $1${NC}"
                    usage
                    exit 1
                fi
                ;;
        esac
    done

    check_llvm_cov

    if [[ "$do_clean" == "true" ]]; then
        clean_coverage
    fi

    # Default top_n for gaps command
    if [[ "$command" == "gaps" && "$top_n" == "10" ]]; then
        top_n="100"  # Show all gaps by default
    fi

    case "$command" in
        ""|summary)
            show_summary "$threshold" "$top_n" "$skip_tests"
            ;;
        gaps)
            show_gaps "$threshold" "$top_n" "$skip_tests"
            ;;
        file)
            show_file "$file_arg" "$skip_tests"
            ;;
        html)
            generate_html "$open_browser" "$skip_tests"
            ;;
        lcov)
            generate_lcov "$skip_tests"
            ;;
        json)
            generate_json "$skip_tests"
            ;;
    esac
}

main "$@"
