#!/usr/bin/env bash
set -euo pipefail

# Release script for the namecode crate.
#
# Runs pre-flight checks, quality gates, and publishes to crates.io.
# Designed to catch every stupid mistake before it becomes permanent.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CRATE="namecode"
CRATE_DIR="$PROJECT_ROOT/$CRATE"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

DRY_RUN=false
SKIP_GATES=false

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Publish the namecode crate to crates.io.

Options:
    --dry-run       Run all checks and cargo publish --dry-run, but don't
                    actually publish or tag
    --skip-gates    Skip quality gates (use when you just ran them)
    -h, --help      Show this help message

The script will:
  1. Pre-flight: clean tree, version not already published, auth ready
  2. Quality gates: fmt, clippy, test, doc, package
  3. Publish: cargo publish, git tag

EOF
}

die() { echo -e "${RED}ABORT:${NC} $1" >&2; exit 1; }
info() { echo -e "${CYAN}::${NC} $1"; }
ok() { echo -e "  ${GREEN}ok${NC} $1"; }
warn() { echo -e "  ${YELLOW}!!${NC} $1"; }
fail() { echo -e "  ${RED}FAIL${NC} $1"; exit 1; }

# ---------- argument parsing ----------

while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)   DRY_RUN=true; shift ;;
        --skip-gates) SKIP_GATES=true; shift ;;
        -h|--help)   usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

cd "$PROJECT_ROOT"

if $DRY_RUN; then
    echo -e "${BOLD}${YELLOW}=== DRY RUN ===${NC}"
    echo ""
fi

# ========================================
# Phase 1: Pre-flight
# ========================================
info "Pre-flight checks"

# 1a. Git working tree must be clean
if [[ -n "$(git status --porcelain)" ]]; then
    echo ""
    git status --short
    echo ""
    fail "working tree is dirty — commit or stash first"
fi
ok "working tree clean"

# 1b. Must be on main branch
BRANCH="$(git branch --show-current)"
if [[ "$BRANCH" != "main" ]]; then
    fail "on branch '${BRANCH}', expected 'main'"
fi
ok "on branch main"

# 1c. Resolve version
LOCAL_VERSION=$(cargo metadata --format-version=1 --no-deps \
    | python3 -c "import sys,json; pkgs=json.load(sys.stdin)['packages']; print([p['version'] for p in pkgs if p['name']=='$CRATE'][0])")
if [[ -z "$LOCAL_VERSION" ]]; then
    fail "could not resolve $CRATE version from cargo metadata"
fi
ok "local version: ${BOLD}${LOCAL_VERSION}${NC}"

# 1d. Check if version is already on crates.io
PUBLISHED_VERSION=$(cargo search "$CRATE" --limit 1 2>/dev/null \
    | grep "^${CRATE} " \
    | sed 's/.*= "\(.*\)".*/\1/' \
    || true)
if [[ "$PUBLISHED_VERSION" == "$LOCAL_VERSION" ]]; then
    fail "version ${LOCAL_VERSION} is already published on crates.io — bump the version first"
fi
if [[ -n "$PUBLISHED_VERSION" ]]; then
    ok "crates.io has ${PUBLISHED_VERSION}, publishing ${LOCAL_VERSION}"
else
    ok "not yet on crates.io — first publish"
fi

# 1e. Check for existing git tag
TAG="${CRATE}/v${LOCAL_VERSION}"
if git rev-parse "$TAG" >/dev/null 2>&1; then
    fail "git tag '${TAG}' already exists"
fi
ok "tag '${TAG}' is available"

# 1f. Verify cargo auth
# cargo publish --dry-run doesn't check auth, so we verify the token exists.
if ! cargo login --help >/dev/null 2>&1; then
    fail "cargo not available"
fi
# Check for credentials file or CARGO_REGISTRY_TOKEN
CARGO_CRED_FILE="${CARGO_HOME:-$HOME/.cargo}/credentials.toml"
CARGO_CRED_FILE_ALT="${CARGO_HOME:-$HOME/.cargo}/credentials"
if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]] && [[ ! -f "$CARGO_CRED_FILE" ]] && [[ ! -f "$CARGO_CRED_FILE_ALT" ]]; then
    if ! $DRY_RUN; then
        fail "no crates.io token found (run 'cargo login' or set CARGO_REGISTRY_TOKEN)"
    else
        warn "no crates.io token found (ok for dry run)"
    fi
else
    ok "cargo auth configured"
fi

echo ""

# ========================================
# Phase 2: Quality gates
# ========================================
if $SKIP_GATES; then
    info "Skipping quality gates (--skip-gates)"
else
    info "Quality gates"

    printf "  fmt: "
    if cargo fmt -p "$CRATE" -- --check >/dev/null 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        echo -e "${RED}FAIL${NC}"
        fail "cargo fmt found formatting issues — run 'cargo fmt' first"
    fi

    printf "  clippy: "
    if cargo clippy -p "$CRATE" --all-targets --quiet -- -D warnings 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        fail "clippy has warnings"
    fi

    printf "  test: "
    test_output=$(cargo test -p "$CRATE" 2>&1)
    test_passed=$(echo "$test_output" | grep -o '[0-9]* passed' | awk '{sum += $1} END {print sum+0}')
    test_failed=$(echo "$test_output" | grep -o '[0-9]* failed' | awk '{sum += $1} END {print sum+0}')
    if [[ "$test_failed" -gt 0 ]]; then
        echo -e "${RED}FAIL${NC}"
        echo "$test_output"
        fail "${test_failed} test(s) failed"
    fi
    echo -e "${GREEN}ok${NC} (${test_passed} passed)"

    printf "  doc: "
    if cargo doc -p "$CRATE" --no-deps --quiet 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        fail "docs failed to build"
    fi

    printf "  package: "
    if cargo package -p "$CRATE" --quiet 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        fail "cargo package failed"
    fi
fi

echo ""

# ========================================
# Phase 3: Publish
# ========================================
info "Publishing ${BOLD}${CRATE} v${LOCAL_VERSION}${NC}"

if $DRY_RUN; then
    echo -e "  ${DIM}cargo publish -p $CRATE --dry-run${NC}"
    cargo publish -p "$CRATE" --dry-run 2>&1 | sed 's/^/  /'
    echo ""
    echo -e "${GREEN}${BOLD}Dry run complete.${NC} Re-run without --dry-run to publish."
else
    echo -e "  ${DIM}cargo publish -p $CRATE${NC}"
    cargo publish -p "$CRATE" 2>&1 | sed 's/^/  /'
    ok "published to crates.io"

    echo ""
    info "Tagging ${TAG}"
    git tag -a "$TAG" -m "$CRATE v$LOCAL_VERSION"
    ok "created tag ${TAG}"

    echo ""
    echo -e "${GREEN}${BOLD}Released ${CRATE} v${LOCAL_VERSION}${NC}"
    echo ""
    echo "  https://crates.io/crates/${CRATE}/${LOCAL_VERSION}"
    echo ""
    echo -e "${DIM}Don't forget:${NC}"
    echo "  git push origin ${TAG}"
fi
