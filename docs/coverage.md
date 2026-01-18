# Coverage Script Manual

The `scripts/coverage.sh` script provides tools for analyzing test coverage in StructFS.

## Integration with Quality Gates

When `./scripts/quality_gates.sh` fails on coverage, it outputs debugging instructions directing you to use this script. The quality gates exclude intentionally untestable files:
- `packages/repl/src/host/terminal.rs` (real terminal I/O)
- `packages/repl/src/lib.rs` (entry point)
- `packages/repl/src/main.rs` (entry point)
- `featherweight/guest` (WASM-only code)

## Quick Start

```bash
# Show overall coverage and top gaps
./scripts/coverage.sh

# List all files below 95% coverage
./scripts/coverage.sh gaps

# See uncovered lines in a specific file
./scripts/coverage.sh file commands.rs

# Open interactive HTML report
./scripts/coverage.sh html
```

## Commands

### Default (Summary)

```bash
./scripts/coverage.sh
```

Shows overall coverage percentage and the top 10 files with the most uncovered lines. Files are color-coded:
- **Green**: At or above target (95%)
- **Yellow**: Between 80-95%
- **Red**: Below 80%
- **Bold Red**: Below 50%

### gaps

```bash
./scripts/coverage.sh gaps
./scripts/coverage.sh gaps -t 90      # Files below 90%
./scripts/coverage.sh gaps -n 5       # Top 5 only
```

Lists all files below the threshold (default 95%), sorted by number of uncovered lines. This helps prioritize where to add tests for maximum impact.

### file

```bash
./scripts/coverage.sh file <path>
./scripts/coverage.sh file commands.rs
./scripts/coverage.sh file packages/repl/src/commands.rs
```

Shows all uncovered lines in a specific file. Accepts partial paths - it will find the first matching file in the coverage data.

Output format:
```
 123 │     uncovered_code_here();
 124 │     more_uncovered_code();
```

### html

```bash
./scripts/coverage.sh html
./scripts/coverage.sh html --no-open
```

Generates an interactive HTML report and opens it in your browser. Use `--no-open` to generate without opening.

### lcov / json

```bash
./scripts/coverage.sh lcov
./scripts/coverage.sh json
```

Generate machine-readable coverage reports for CI integration.

## Options

| Option | Description |
|--------|-------------|
| `-h, --help` | Show help message |
| `-t, --threshold N` | Coverage threshold percentage (default: 95) |
| `-n, --top N` | Limit output to top N files |
| `--no-open` | Don't open browser for HTML report |
| `--clean` | Clean coverage data before running |
| `--skip-tests` | Use cached coverage data (faster) |

## Workflow Examples

### Finding where to add tests

```bash
# 1. See overall status and biggest gaps
./scripts/coverage.sh

# 2. Drill into a specific file
./scripts/coverage.sh file commands.rs

# 3. Write tests for uncovered lines, then verify
./scripts/coverage.sh --clean
```

### Checking progress toward a target

```bash
# See what's left to reach 95%
./scripts/coverage.sh gaps -t 95

# Or for a different target
./scripts/coverage.sh gaps -t 90
```

### Fast iteration during test writing

```bash
# Run tests once to populate cache
./scripts/coverage.sh

# Check specific file without re-running tests
./scripts/coverage.sh file myfile.rs --skip-tests
```

## Understanding the Output

The coverage percentage shown is **region coverage** (code regions executed vs total). This is more granular than line coverage and accounts for branches within lines.

Files with 0% coverage that are intentionally untestable (like `terminal.rs` for real terminal I/O) should be excluded from coverage targets in CI.

## How It Works

The script runs tests once and caches two outputs:
- `target/coverage/coverage_summary.txt` - per-file stats and totals
- `target/coverage/coverage_detail.txt` - line-by-line coverage

Subsequent commands with `--skip-tests` read from cache (~0.2s) instead of re-running tests (~15s).

Use `--clean` to clear the cache and re-run tests.

## Requirements

- `cargo-llvm-cov` (automatically installed if missing)
- `bc` (for floating point comparison)
