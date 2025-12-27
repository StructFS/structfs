# StructFS Architecture Fix Plans

Decision: **Everything is a store.** These plans fix the abstractions that currently break that principle.

## Plans

1. [Fix Unmount](./01-fix-unmount.md) - Actually remove stores from overlay on unmount
2. [Idempotent HTTP Broker](./02-idempotent-http-broker.md) - Reads don't destroy state
3. [Filesystem Position](./03-filesystem-position.md) - Position as addressable state
4. [Registers as Store](./04-registers-as-store.md) - Mount at `/ctx/registers/`
5. [Error Type Cleanup](./05-error-cleanup.md) - Structured, contextual errors
6. [Document Mutability](./06-document-mutability.md) - Explain `&mut self` decision
7. [Docs Router](./07-docs-router.md) - Auto-discover and route to store docs

## Implementation Order

Recommended sequence based on dependencies:

1. **Error types** (Plan 5) - Foundation for cleaner error handling
2. **Unmount fix** (Plan 1) - Simple, self-contained, **required for Plan 7**
3. **Docs router** (Plan 7) - Adds redirects to OverlayStore, enables auto-discovery
4. **HTTP broker** (Plan 2) - Medium complexity, enables testing patterns
5. **Filesystem position** (Plan 3) - Builds on error types, broker patterns
6. **Registers as store** (Plan 4) - Larger refactor, can use docs router for help
7. **Document mutability** (Plan 6) - Documentation, do alongside others

## Testing Strategy

Each plan should include:

1. **Unit tests** for new/changed functions
2. **Integration tests** for end-to-end workflows
3. **Regression tests** to ensure existing behavior preserved
4. **Edge case tests** for error paths
