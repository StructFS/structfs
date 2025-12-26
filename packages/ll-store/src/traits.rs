//! Core traits for the LL layer.

use bytes::Bytes;

use crate::LLError;

/// An owned path at the LL level - a sequence of byte components.
///
/// Each component is a `Bytes`, which is reference-counted and supports
/// zero-copy slicing. No validation is performed on the components -
/// they are opaque byte sequences.
pub type LLPath = Vec<Bytes>;

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
