# Plan 1: Fix Unmount - ✅ COMPLETED

## Status: DONE (2025-12-27)

This plan has been fully implemented.

## What Was Implemented

### OverlayStore

- `PathTrie<RouteTarget>` replaces the old `Vec<(Path, Store)>` structure
- `unmount(path)` calls `trie.remove_subtree(path)` for clean removal
- O(k) operations where k is path depth

### MountStore

- `unmount(name)` delegates to `OverlayStore.unmount()`
- Also calls `remove_redirects_for_mount(name)` for cascade cleanup
- Tracks mounts in `BTreeMap<String, MountConfig>` for serialization

### Cascade Behavior

When unmounting a store:
1. Remove the store from the routing trie
2. Remove any redirects that were created by that mount (e.g., docs redirects)
3. Update the HelpStore index (handled by `StoreContext.refresh_help_state()`)

## Implementation Files

- `packages/core-store/src/path_trie.rs` - PathTrie<T> with O(k) operations
- `packages/core-store/src/overlay_store.rs` - OverlayStore using PathTrie
- `packages/core-store/src/mount_store.rs` - MountStore with cascade unmount
- `packages/repl/src/store_context.rs` - StoreContext with help state refresh

## Tests

All passing. See test modules in each file.
