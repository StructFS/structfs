//! Core traits for the LL layer.

use bytes::Bytes;

use crate::LLError;

/// An owned path at the LL level: a sequence of **opaque byte components**.
///
/// This is the low-level path contract, made concrete as a type rather than a
/// bare `Vec<Bytes>` alias. Each component is a `Bytes` (reference-counted,
/// zero-copy sliceable). **No validation is performed** — components are
/// arbitrary byte sequences with no UTF-8 or grammar requirement.
///
/// The high-level `Path` (in `structfs_core_store`) is a *validated refinement*
/// of this type: every `Path` widens to an `LLPath` for free, and an `LLPath`
/// narrows to a `Path` only by validation. Keeping the two contracts as
/// distinct types is what lets the widening direction stay zero-cost and puts
/// validation at exactly one narrowing boundary.
///
/// The inner representation is intentionally private so it can later change
/// (e.g. to a single flat buffer with component offsets) without touching call
/// sites — `Deref<Target = [Bytes]>` plus the iterator impls keep the common
/// read/iterate/collect patterns working regardless of the backing layout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LLPath(Vec<Bytes>);

impl LLPath {
    /// An empty path (zero components).
    pub fn new() -> Self {
        LLPath(Vec::new())
    }

    /// Build a path from owned byte components.
    pub fn from_components(components: Vec<Bytes>) -> Self {
        LLPath(components)
    }

    /// The components as a slice.
    pub fn components(&self) -> &[Bytes] {
        &self.0
    }

    /// Consume into the underlying component vector.
    pub fn into_components(self) -> Vec<Bytes> {
        self.0
    }

    /// Append a component.
    pub fn push(&mut self, component: Bytes) {
        self.0.push(component);
    }

    /// Borrow each component as a byte slice, for passing to the `&[&[u8]]`
    /// read/write interface without copying any bytes.
    pub fn as_byte_refs(&self) -> Vec<&[u8]> {
        self.0.iter().map(|c| c.as_ref()).collect()
    }
}

impl std::ops::Deref for LLPath {
    type Target = [Bytes];
    fn deref(&self) -> &[Bytes] {
        &self.0
    }
}

impl FromIterator<Bytes> for LLPath {
    fn from_iter<I: IntoIterator<Item = Bytes>>(iter: I) -> Self {
        LLPath(iter.into_iter().collect())
    }
}

impl IntoIterator for LLPath {
    type Item = Bytes;
    type IntoIter = std::vec::IntoIter<Bytes>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a LLPath {
    type Item = &'a Bytes;
    type IntoIter = std::slice::Iter<'a, Bytes>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<Vec<Bytes>> for LLPath {
    fn from(components: Vec<Bytes>) -> Self {
        LLPath(components)
    }
}

/// Read bytes from a path.
///
/// This is the lowest-level read interface. Paths are just byte sequences,
/// and the returned data is just bytes. No parsing, no validation.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn LLReader>`.
pub trait LLReader: Send + Sync {
    /// Read raw bytes from path components.
    ///
    /// # Arguments
    ///
    /// * `path` - A slice of byte slices representing path components.
    ///   No validation is performed - components are opaque bytes.
    ///
    /// # Returns
    ///
    /// * `Ok(None)` - The path does not exist (not an error condition).
    /// * `Ok(Some(bytes))` - The data at the path.
    /// * `Err(LLError)` - A transport or system error occurred.
    ///
    /// # Example
    ///
    /// ```rust
    /// use structfs_ll_store::{LLReader, LLError};
    /// use bytes::Bytes;
    ///
    /// fn read_user(store: &mut dyn LLReader, user_id: &str) -> Result<Option<Bytes>, LLError> {
    ///     store.ll_read(&[b"users", user_id.as_bytes()])
    /// }
    /// ```
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError>;
}

/// Write bytes to a path.
///
/// This is the lowest-level write interface. Paths and data are just bytes.
/// No parsing, no validation.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn LLWriter>`.
pub trait LLWriter: Send + Sync {
    /// Write raw bytes to path components.
    ///
    /// # Arguments
    ///
    /// * `path` - A slice of byte slices representing path components.
    /// * `data` - The bytes to write.
    ///
    /// # Returns
    ///
    /// The "result path" as a sequence of byte components. This may be:
    /// - The same as the input path (for simple stores)
    /// - A different path (e.g., a generated ID, a handle for async operations)
    ///
    /// # Example
    ///
    /// ```rust
    /// use structfs_ll_store::{LLWriter, LLPath, LLError};
    /// use bytes::Bytes;
    ///
    /// fn create_user(store: &mut dyn LLWriter, data: &[u8]) -> Result<LLPath, LLError> {
    ///     store.ll_write(&[b"users"], Bytes::copy_from_slice(data))
    /// }
    /// ```
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError>;
}

/// Combined read/write at the LL level.
///
/// This is a convenience trait for stores that support both reading and writing.
/// It is automatically implemented for any type that implements both `LLReader`
/// and `LLWriter`.
pub trait LLStore: LLReader + LLWriter {}
impl<T: LLReader + LLWriter> LLStore for T {}

// Blanket implementations for references and boxes

impl<T: LLReader + ?Sized> LLReader for &mut T {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        (*self).ll_read(path)
    }
}

impl<T: LLWriter + ?Sized> LLWriter for &mut T {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        (*self).ll_write(path, data)
    }
}

impl<T: LLReader + ?Sized> LLReader for Box<T> {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        self.as_mut().ll_read(path)
    }
}

impl<T: LLWriter + ?Sized> LLWriter for Box<T> {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        self.as_mut().ll_write(path, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A simple in-memory LL store for testing.
    struct TestLLStore {
        data: HashMap<Vec<Vec<u8>>, Bytes>,
    }

    impl TestLLStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl LLReader for TestLLStore {
        fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            Ok(self.data.get(&key).cloned())
        }
    }

    impl LLWriter for TestLLStore {
        fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            self.data.insert(key, data);
            Ok(path.iter().map(|c| Bytes::copy_from_slice(c)).collect())
        }
    }

    #[test]
    fn basic_read_write_works() {
        let mut store = TestLLStore::new();

        // Write some data
        let path = &[b"users".as_slice(), b"123".as_slice()];
        let data = Bytes::from_static(b"hello world");
        store.ll_write(path, data.clone()).unwrap();

        // Read it back
        let result = store.ll_read(path).unwrap();
        assert_eq!(result, Some(data));

        // Read non-existent path
        let result = store.ll_read(&[b"nonexistent"]).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn object_safety_works() {
        let mut store = TestLLStore::new();
        let boxed: &mut dyn LLStore = &mut store;

        boxed
            .ll_write(&[b"test"], Bytes::from_static(b"data"))
            .unwrap();
        let result = boxed.ll_read(&[b"test"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"data")));
    }

    #[test]
    fn mut_ref_blanket_impl_works() {
        let mut store = TestLLStore::new();
        let store_ref: &mut TestLLStore = &mut store;

        store_ref
            .ll_write(&[b"ref_test"], Bytes::from_static(b"ref_data"))
            .unwrap();
        let result = store_ref.ll_read(&[b"ref_test"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"ref_data")));
    }

    #[test]
    fn box_blanket_impl_works() {
        let store = TestLLStore::new();
        let mut boxed: Box<TestLLStore> = Box::new(store);

        boxed
            .ll_write(&[b"box_test"], Bytes::from_static(b"box_data"))
            .unwrap();
        let result = boxed.ll_read(&[b"box_test"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"box_data")));
    }

    #[test]
    fn box_dyn_works() {
        let store = TestLLStore::new();
        let mut boxed: Box<dyn LLStore> = Box::new(store);

        boxed
            .ll_write(&[b"dyn_test"], Bytes::from_static(b"dyn_data"))
            .unwrap();
        let result = boxed.ll_read(&[b"dyn_test"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"dyn_data")));
    }
}
