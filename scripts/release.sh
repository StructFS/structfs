#!/usr/bin/env bash
set -euo pipefail

# Workspace-wide release script for StructFS.
#
# Publishes any/all workspace crates to crates.io in correct dependency order.
# Designed to catch every stupid mistake before it becomes permanent.
# Re-runnable: already-published crates are auto-detected and skipped.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

DRY_RUN=false
SKIP_GATES=false
PROPAGATION_DELAY=30
declare -a ONLY_CRATES=()
declare -a EXCLUDE_CRATES=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Publish workspace crates to crates.io in dependency order.

Options:
    --dry-run              Run all checks and cargo publish --dry-run, but
                           don't actually publish or tag
    --skip-gates           Skip quality gates (use when you just ran them)
    --crate NAME           Only publish named crate(s); repeatable
    --exclude NAME         Skip named crate(s); repeatable
    --propagation-delay N  Seconds between publishes (default: 30)
    -h, --help             Show this help message

The script will:
  1. Pre-flight: clean tree, compute publish order, check versions
  2. Quality gates: fmt, clippy, test, doc, package (workspace-wide)
  3. Publish: cargo publish per crate in dependency order, git tag each

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
        --dry-run)            DRY_RUN=true; shift ;;
        --skip-gates)         SKIP_GATES=true; shift ;;
        --crate)              ONLY_CRATES+=("$2"); shift 2 ;;
        --exclude)            EXCLUDE_CRATES+=("$2"); shift 2 ;;
        --propagation-delay)  PROPAGATION_DELAY="$2"; shift 2 ;;
        -h|--help)            usage; exit 0 ;;
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

# 1a. Require jq
if ! command -v jq &>/dev/null; then
    fail "jq is required but not found — install with: brew install jq (macOS) or apt install jq (Linux)"
fi
ok "jq available"

# 1b. Require tsort
if ! command -v tsort &>/dev/null; then
    fail "tsort is required but not found (should be in coreutils)"
fi
ok "tsort available"

# 1c. Git working tree must be clean
if [[ -n "$(git status --porcelain)" ]]; then
    echo ""
    git status --short
    echo ""
    fail "working tree is dirty — commit or stash first"
fi
ok "working tree clean"

# 1d. Must be on main branch
BRANCH="$(git branch --show-current)"
if [[ "$BRANCH" != "main" ]]; then
    fail "on branch '${BRANCH}', expected 'main'"
fi
ok "on branch main"

# 1e. Verify cargo auth
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

# 1f. Compute publish order via cargo metadata + jq + tsort
info "Computing publish order"

METADATA=$(cargo metadata --format-version=1 --no-deps)

# Extract dependency edges for publishable workspace crates
EDGES=$(echo "$METADATA" | jq -r '
  [.workspace_default_members[] | split(" ")[0]] as $members |
  [.packages[]
    | select((.id | split(" ")[0]) as $id | $members | index($id))
    | select((.publish // []) | length == 0)
  ] |
  . as $pkgs |
  [.[] | .name] as $names |
  [.[] | {name: .name, deps: [.dependencies[] | select(.path) | .name]
    | map(select(. as $d | $names | index($d)))}] |
  .[] |
  if (.deps | length) == 0
    then "\(.name)\t\(.name)"
    else .deps[] as $d | "\($d)\t\(.name)"
  end
')

if [[ -z "$EDGES" ]]; then
    fail "no publishable crates found in workspace"
fi

# Topological sort
ORDERED=$(echo "$EDGES" | tsort) || die "circular dependency detected in workspace"

# Build name→version lookup
declare -A VERSION_MAP
while IFS=$'\t' read -r name version; do
    VERSION_MAP["$name"]="$version"
done < <(echo "$METADATA" | jq -r '.packages[] | "\(.name)\t\(.version)"')

# Apply --crate filter
if [[ ${#ONLY_CRATES[@]} -gt 0 ]]; then
    FILTERED=""
    for crate in $ORDERED; do
        for want in "${ONLY_CRATES[@]}"; do
            if [[ "$crate" == "$want" ]]; then
                FILTERED="${FILTERED}${crate}"$'\n'
                break
            fi
        done
    done
    ORDERED="$FILTERED"
fi

# Apply --exclude filter
if [[ ${#EXCLUDE_CRATES[@]} -gt 0 ]]; then
    FILTERED=""
    for crate in $ORDERED; do
        skip=false
        for excl in "${EXCLUDE_CRATES[@]}"; do
            if [[ "$crate" == "$excl" ]]; then
                skip=true
                break
            fi
        done
        if ! $skip; then
            FILTERED="${FILTERED}${crate}"$'\n'
        fi
    done
    ORDERED="$FILTERED"
fi

# Remove trailing newline
ORDERED=$(echo "$ORDERED" | sed '/^$/d')

if [[ -z "$ORDERED" ]]; then
    fail "no crates remain after filters"
fi

# 1g. Per-crate version checks; build final publish list
info "Checking versions"

declare -a PUBLISH_NAMES=()
declare -a PUBLISH_VERSIONS=()
declare -a SKIP_NAMES=()

while IFS= read -r crate; do
    version="${VERSION_MAP[$crate]}"
    tag="${crate}/v${version}"

    # Check crates.io
    published=$(cargo search "$crate" --limit 1 2>/dev/null \
        | grep "^${crate} " \
        | sed 's/.*= "\(.*\)".*/\1/' \
        || true)

    if [[ "$published" == "$version" ]]; then
        ok "${crate} v${version} — already published, skipping"
        SKIP_NAMES+=("$crate")
        continue
    fi

    # Check git tag
    if git rev-parse "$tag" >/dev/null 2>&1; then
        ok "${crate} v${version} — tag exists, skipping"
        SKIP_NAMES+=("$crate")
        continue
    fi

    if [[ -n "$published" ]]; then
        ok "${crate}: crates.io has ${published}, will publish ${BOLD}${version}${NC}"
    else
        ok "${crate}: first publish — ${BOLD}v${version}${NC}"
    fi

    PUBLISH_NAMES+=("$crate")
    PUBLISH_VERSIONS+=("$version")
done <<< "$ORDERED"

if [[ ${#PUBLISH_NAMES[@]} -eq 0 ]]; then
    echo ""
    info "Nothing to publish — all crates are up to date."
    exit 0
fi

# Print publish plan
echo ""
info "Publish plan (${#PUBLISH_NAMES[@]} crate(s)):"
for i in "${!PUBLISH_NAMES[@]}"; do
    echo -e "  $((i+1)). ${BOLD}${PUBLISH_NAMES[$i]}${NC} v${PUBLISH_VERSIONS[$i]}"
done
if [[ ${#SKIP_NAMES[@]} -gt 0 ]]; then
    echo -e "  ${DIM}(skipping: ${SKIP_NAMES[*]})${NC}"
fi
echo ""

# ========================================
# Phase 2: Quality gates (workspace-wide)
# ========================================
if $SKIP_GATES; then
    info "Skipping quality gates (--skip-gates)"
else
    info "Quality gates"

    printf "  fmt: "
    if cargo fmt --all -- --check >/dev/null 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        echo -e "${RED}FAIL${NC}"
        fail "cargo fmt found formatting issues — run 'cargo fmt' first"
    fi

    printf "  clippy: "
    if cargo clippy --workspace --all-targets --quiet -- -D warnings 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        fail "clippy has warnings"
    fi

    printf "  test: "
    test_output=$(cargo test --workspace 2>&1)
    test_passed=$(echo "$test_output" | grep -o '[0-9]* passed' | awk '{sum += $1} END {print sum+0}')
    test_failed=$(echo "$test_output" | grep -o '[0-9]* failed' | awk '{sum += $1} END {print sum+0}')
    if [[ "$test_failed" -gt 0 ]]; then
        echo -e "${RED}FAIL${NC}"
        echo "$test_output"
        fail "${test_failed} test(s) failed"
    fi
    echo -e "${GREEN}ok${NC} (${test_passed} passed)"

    printf "  doc: "
    if cargo doc --workspace --no-deps --quiet 2>&1; then
        echo -e "${GREEN}ok${NC}"
    else
        fail "docs failed to build"
    fi

    for i in "${!PUBLISH_NAMES[@]}"; do
        crate="${PUBLISH_NAMES[$i]}"
        printf "  package %s: " "$crate"
        if cargo package -p "$crate" --quiet 2>&1; then
            echo -e "${GREEN}ok${NC}"
        else
            fail "cargo package failed for ${crate}"
        fi
    done
fi

echo ""

# ========================================
# Phase 3: Publish loop
# ========================================
info "Publishing ${#PUBLISH_NAMES[@]} crate(s)"
echo ""

declare -a PUBLISHED_TAGS=()
total=${#PUBLISH_NAMES[@]}

for i in "${!PUBLISH_NAMES[@]}"; do
    crate="${PUBLISH_NAMES[$i]}"
    version="${PUBLISH_VERSIONS[$i]}"
    tag="${crate}/v${version}"
    step=$((i+1))

    info "[${step}/${total}] ${BOLD}${crate} v${version}${NC}"

    if $DRY_RUN; then
        echo -e "  ${DIM}cargo publish -p ${crate} --dry-run${NC}"
        cargo publish -p "$crate" --dry-run 2>&1 | sed 's/^/  /'
    else
        echo -e "  ${DIM}cargo publish -p ${crate}${NC}"
        cargo publish -p "$crate" 2>&1 | sed 's/^/  /'
        ok "published to crates.io"

        git tag -a "$tag" -m "${crate} v${version}"
        ok "created tag ${tag}"

        PUBLISHED_TAGS+=("$tag")
    fi

    # Propagation delay between publishes (skip after last, skip for dry-run)
    if [[ $step -lt $total ]] && ! $DRY_RUN; then
        echo -e "  ${DIM}waiting ${PROPAGATION_DELAY}s for crates.io propagation...${NC}"
        sleep "$PROPAGATION_DELAY"
    fi

    echo ""
done

# ========================================
# Summary
# ========================================
if $DRY_RUN; then
    echo -e "${GREEN}${BOLD}Dry run complete.${NC} Re-run without --dry-run to publish."
else
    echo -e "${GREEN}${BOLD}Released ${total} crate(s):${NC}"
    echo ""
    for i in "${!PUBLISH_NAMES[@]}"; do
        echo "  https://crates.io/crates/${PUBLISH_NAMES[$i]}/${PUBLISH_VERSIONS[$i]}"
    done
    echo ""
    if [[ ${#PUBLISHED_TAGS[@]} -gt 0 ]]; then
        echo -e "${DIM}Don't forget to push tags:${NC}"
        echo "  git push origin ${PUBLISHED_TAGS[*]}"
    fi
fi
