#!/usr/bin/env bash
set -euo pipefail

MIN_COVERAGE=90

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
    cov_json=$(cargo llvm-cov --workspace --json 2>/dev/null)
    # Extract line coverage percentage from totals
    coverage=$(echo "$cov_json" | grep -o '"lines":{"count":[0-9]*,"covered":[0-9]*' | head -1 | \
        awk -F'[:,]' '{if ($3 > 0) printf "%.1f", ($5 / $3) * 100; else print "0"}')

    if [[ -z "$coverage" ]]; then
        echo "error (could not parse coverage)"
        exit 1
    fi

    # Compare as integers (bash doesn't do float comparison)
    cov_int=${coverage%.*}
    if [[ "$cov_int" -lt "$MIN_COVERAGE" ]]; then
        echo "FAILED (${coverage}% < ${MIN_COVERAGE}%)"
        exit 1
    else
        echo "ok (${coverage}%)"
    fi
fi
