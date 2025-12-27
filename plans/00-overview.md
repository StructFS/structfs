# StructFS Architecture Fix Plans

Decision: **Everything is a store.** These plans fix the abstractions that currently break that principle.

## Status (Updated 2025-12-27)

### Completed

1. **[Fix Unmount](./01-fix-unmount.md)** - ✅ DONE
   - `OverlayStore.unmount()` correctly removes from the PathTrie
   - `MountStore.unmount()` also cascade-removes redirects

2. **[Overlay Trie](./06-overlay-trie.md)** - ✅ DONE
   - `PathTrie<T>` implemented in `packages/core-store/src/path_trie.rs`
   - `OverlayStore` wraps `PathTrie<RouteTarget>` with fallthrough routing

3. **[Docs Router](./07-docs-router.md)** + **[Completion](./07b-docs-router-completion.md)** - ✅ DONE
   - `RouteTarget` enum with `Store` and `Redirect` variants
   - Cycle detection via visited set
   - `HelpStore` as pure aggregator with search, meta, topic listing
   - `ReplDocsStore` provides REPL documentation
   - Mount-time discovery creates redirects from `/ctx/help/{name}` to `{mount}/docs`
   - Cascade unmount removes redirects

4. **[Idempotent HTTP Broker](./02-idempotent-http-broker.md)** - ✅ DONE
   - `SyncRequestHandle` caches response after first read
   - Subsequent reads return cached result (idempotent)
   - Explicit deletion via `write null`

5. **[Filesystem Position](./03-filesystem-position.md)** - ✅ DONE
   - `FileHandle` explicitly tracks `position: u64`
   - Position queryable at `/handles/{id}/position`
   - Read/write at offset via `/handles/{id}/at/{offset}`

### In Progress / Still Relevant

6. **[Registers as Store](./04-registers-as-store.md)** - Partially done
   - `RegisterStore` exists and implements `Reader`/`Writer`
   - BUT it's still embedded in `StoreContext`, not mounted at `/ctx/registers/`
   - `@` syntax still handled specially in commands, not as sugar

7. **[Error Type Cleanup](./05-error-cleanup.md)** - ✅ DONE
   - `structfs_core_store::Error` has no `Other` variant
   - Replaced with structured `Error::Store { store, operation, message }`
   - HTTP crate's `Error::Other` is intentional (HTTP-specific, converts to `CoreError::store()`)

8. **[Document Mutability](./06-document-mutability.md)** - Still needed
   - Documentation task explaining `&mut self` decision

## Implementation Priority

**7 of 8 plans are complete.** Remaining work:

1. **Registers as Store** (Plan 4) - Mount at `/ctx/registers/`, make `@` syntax pure sugar
2. **Document Mutability** (Plan 6) - Write the documentation

## Testing Strategy

Each plan should include:

1. **Unit tests** for new/changed functions
2. **Integration tests** for end-to-end workflows
3. **Regression tests** to ensure existing behavior preserved
4. **Edge case tests** for error paths
