# Plan 7: Docs Router - ✅ COMPLETED

## Status: DONE (2025-12-27)

This plan and its completion (07b) have been fully implemented.

## What Was Implemented

### Docs Protocol

Stores provide documentation at a `docs` sub-path:
- `{mount}/docs` - Root manifest with title, description, children, keywords
- `{mount}/docs/{topic}` - Subtopics

### Mount-Time Discovery

When mounting a store, `MountStore` probes for docs:

```rust
fn discover_and_redirect_docs(&mut self, name: &str, mount_path: &Path) {
    let docs_path = mount_path.join(&path!("docs"));
    if self.overlay.read(&docs_path).ok().flatten().is_some() {
        let help_path = path!("ctx/help").join(&help_suffix);
        self.overlay.add_redirect(help_path, docs_path, ReadOnly, Some(name));
    }
}
```

### HelpStore as Aggregator

`HelpStore` is a pure aggregator - it holds NO content:

- `read /ctx/help` - List all indexed topics
- `read /ctx/help/meta` - All redirect mappings
- `read /ctx/help/meta/{topic}` - Single redirect info
- `read /ctx/help/search/{query}` - Search across all topics
- `read /ctx/help/{topic}` - Redirected to `{mount}/docs` by OverlayStore

### Dynamic Index Updates

`StoreContext` holds a `HelpStoreHandle` (Arc<RwLock<HelpStoreState>>):
- `mount()` calls `refresh_help_state()` to update index
- `unmount()` also calls `refresh_help_state()` to remove topics

### Cascade Unmount

When unmounting a store:
1. `MountStore.unmount()` removes the store
2. Calls `remove_redirects_for_mount(name)` to remove docs redirect
3. `StoreContext.refresh_help_state()` rebuilds the index

## Implementation Files

- `packages/core-store/src/overlay_store.rs` - RouteTarget, redirect handling
- `packages/core-store/src/mount_store.rs` - discover_and_redirect_docs
- `packages/repl/src/help_store.rs` - HelpStore, DocsIndex, DocsManifest
- `packages/repl/src/repl_docs_store.rs` - ReplDocsStore
- `packages/repl/src/store_context.rs` - Dynamic index updates

## Stores with Docs

- `SysStore` at `/ctx/sys` provides `/ctx/sys/docs`
- `ReplDocsStore` at `/ctx/repl` provides `/ctx/repl/docs`

## Tests

Comprehensive test coverage. All passing.
