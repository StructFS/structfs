# Plan 1: Fix Unmount

## Problem

`MountStore::unmount()` removes the mount from the tracking `BTreeMap` but does NOT remove the store from `OverlayStore.routes`. The mount is "forgotten" but still routes traffic.

```rust
// Current broken implementation (mount_store.rs:111-126)
pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
    self.mounts.remove(name);  // Removes from tracking
    // But overlay.routes still has the store!
    Ok(())
}
```

## Solution

Add `remove_layer()` to `OverlayStore`, then call it from `unmount()`.

## Implementation Steps

### Step 1: Add `remove_layer` to OverlayStore

**File:** `packages/core-store/src/overlay_store.rs`

```rust
/// Remove a layer by its exact prefix path.
/// Returns the removed store if found, None otherwise.
pub fn remove_layer(&mut self, prefix: &Path) -> Option<StoreBox> {
    // Use rposition to find the last (highest priority) matching prefix
    if let Some(pos) = self.routes.iter().rposition(|(p, _)| p == prefix) {
        Some(self.routes.remove(pos).1)
    } else {
        None
    }
}
```

### Step 2: Update `unmount()` in MountStore

**File:** `packages/core-store/src/mount_store.rs`

```rust
pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
    if !self.mounts.contains_key(name) {
        return Err(Error::Other {
            message: format!("No mount at '{}'", name),
        });
    }

    // Parse the mount path
    let mount_path = Path::parse(name).map_err(Error::Path)?;

    // Remove from overlay (the actual routing)
    self.overlay.remove_layer(&mount_path);

    // Remove from tracking (the metadata)
    self.mounts.remove(name);

    Ok(())
}
```

### Step 3: Add comprehensive tests

```rust
#[test]
fn unmount_removes_from_overlay() {
    let mut store = MountStore::new(TestFactory);
    store.mount("data", MountConfig::Memory).unwrap();

    // Write something
    store.write(&path!("data/key"), Record::parsed(Value::Integer(42))).unwrap();

    // Verify it's readable
    let result = store.read(&path!("data/key")).unwrap();
    assert!(result.is_some());

    // Unmount
    store.unmount("data").unwrap();

    // Verify it's no longer routable (should return NoRoute error or None)
    let result = store.read(&path!("data/key"));
    assert!(result.is_err() || result.unwrap().is_none());
}

#[test]
fn unmount_allows_remount() {
    let mut store = MountStore::new(TestFactory);
    store.mount("data", MountConfig::Memory).unwrap();
    store.write(&path!("data/key"), Record::parsed(Value::Integer(1))).unwrap();

    store.unmount("data").unwrap();
    store.mount("data", MountConfig::Memory).unwrap();

    // New mount should be empty
    let result = store.read(&path!("data/key")).unwrap();
    assert!(result.is_none());
}

#[test]
fn unmount_priority_preserved() {
    let mut store = MountStore::new(TestFactory);

    // Mount two stores at overlapping paths
    store.mount("data", MountConfig::Memory).unwrap();
    store.mount("data/nested", MountConfig::Memory).unwrap();

    // Write to nested
    store.write(&path!("data/nested/key"), Record::parsed(Value::Integer(1))).unwrap();

    // Unmount nested
    store.unmount("data/nested").unwrap();

    // data should still work
    store.write(&path!("data/other"), Record::parsed(Value::Integer(2))).unwrap();
    let result = store.read(&path!("data/other")).unwrap();
    assert!(result.is_some());
}
```

## Files Changed

- `packages/core-store/src/overlay_store.rs` - Add `remove_layer()`
- `packages/core-store/src/mount_store.rs` - Fix `unmount()`
- `packages/core-store/src/mount_store.rs` - Add tests

## Complexity

Low - Two small changes, well-contained.
