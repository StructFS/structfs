#!/usr/bin/env bash
set -euo pipefail

# Quality gates for StructFS
#
# IMPORTANT: If you modify this script, ensure coverage instructions
# remain useful for Claude Code debugging sessions.

MIN_COVERAGE=90

# Files that are intentionally untestable (real I/O, entry points)
EXCLUDED_FILES=(
    "packages/repl/src/host/terminal.rs"
    "packages/repl/src/lib.rs"
    "packages/repl/src/main.rs"
    "featherweight/guest"
)

printf "fmt: "
if cargo fmt --all -- --check >/dev/null 2>&1; then
    echo "ok"
else
    echo "reformatted"
    cargo fmt --all --quiet
fi

printf "clippy: "
cargo clippy --workspace --all-targets --quiet -- -D warnings && echo "ok"

printf "test: "
test_output=$(cargo test --workspace 2>&1)
# Count passed/failed/ignored from all "test result:" lines
passed=$(echo "$test_output" | grep -o '[0-9]* passed' | awk '{sum += $1} END {print sum+0}')
failed=$(echo "$test_output" | grep -o '[0-9]* failed' | awk '{sum += $1} END {print sum+0}')
ignored=$(echo "$test_output" | grep -o '[0-9]* ignored' | awk '{sum += $1} END {print sum+0}')

if [[ "$failed" -gt 0 ]]; then
    echo "FAILED ($passed passed, $failed failed, $ignored ignored)"
    echo "$test_output"
    exit 1
else
    echo "ok ($passed passed, $ignored ignored)"
fi

printf "coverage: "
if ! command -v cargo-llvm-cov &> /dev/null; then
    echo "skipped (cargo-llvm-cov not installed)"
else
    # Build exclusion regex from EXCLUDED_FILES
    exclude_regex=$(IFS='|'; echo "${EXCLUDED_FILES[*]}")

    # Run coverage and cache results for debugging
    COVERAGE_DIR="target/coverage"
    mkdir -p "$COVERAGE_DIR"

    cov_output=$(cargo llvm-cov --workspace --ignore-filename-regex "$exclude_regex" 2>&1)
    echo "$cov_output" > "$COVERAGE_DIR/coverage_summary.txt"

    # Extract coverage from TOTAL line
    total_line=$(echo "$cov_output" | grep "^TOTAL" || true)
    if [[ -z "$total_line" ]]; then
        echo "error (could not find TOTAL line)"
        exit 1
    fi

    # TOTAL format: TOTAL  regions  missed  pct%  ...
    coverage=$(echo "$total_line" | awk '{print $4}' | tr -d '%')

    if [[ -z "$coverage" ]]; then
        echo "error (could not parse coverage)"
        exit 1
    fi

    # Compare as integers (bash doesn't do float comparison)
    cov_int=${coverage%.*}
    if [[ "$cov_int" -lt "$MIN_COVERAGE" ]]; then
        echo "FAILED (${coverage}% < ${MIN_COVERAGE}%)"
        echo ""
        echo "=========================================="
        echo "COVERAGE FAILURE - DEBUGGING INSTRUCTIONS"
        echo "=========================================="
        echo ""
        echo "Current coverage: ${coverage}% (target: ${MIN_COVERAGE}%)"
        echo ""
        echo "To diagnose and fix coverage gaps efficiently:"
        echo ""
        echo "1. See top coverage gaps (sorted by impact):"
        echo "   ./scripts/coverage.sh gaps --skip-tests"
        echo ""
        echo "2. See uncovered lines in a specific file:"
        echo "   ./scripts/coverage.sh file <filename> --skip-tests"
        echo ""
        echo "3. Focus on files with most uncovered lines first."
        echo "   Exclude these intentionally untestable files:"
        for f in "${EXCLUDED_FILES[@]}"; do
            echo "   - $f"
        done
        echo ""
        echo "4. Common patterns needing tests:"
        echo "   - Error handling branches (Err(e) => ...)"
        echo "   - Edge cases in parsing/validation"
        echo "   - Fallback/default branches"
        echo ""
        echo "5. After adding tests, verify with:"
        echo "   ./scripts/coverage.sh --clean"
        echo ""
        echo "TOP GAPS:"
        echo "$cov_output" | grep -E "^(packages/|featherweight/)" | \
            awk -v threshold="$MIN_COVERAGE" '
            {
                file = $1
                missed = $3
                pct = $4
                gsub(/%/, "", pct)
                if (pct + 0 < threshold + 0) {
                    printf "  %6s  %4d uncovered  %s\n", pct "%", missed, file
                }
            }' | sort -t'%' -k1 -n | head -10
        echo ""
        exit 1
    else
        echo "ok (${coverage}%)"
    fi
fi
