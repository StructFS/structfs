// Legacy implementations (to be deprecated)
pub mod in_memory;
pub mod json_utils;
pub mod local_disk;

// New core-store based implementations
pub mod core_in_memory;
pub mod value_utils;

// Legacy re-exports
pub use in_memory::SerdeJSONInMemoryStore;
pub use local_disk::JSONLocalStore;
pub use structfs_store::store;
pub use structfs_store::{Path, PathError};

// New architecture re-exports
pub use core_in_memory::InMemoryStore;
pub use structfs_core_store::{
    path as core_path, Error as CoreError, Path as CorePath, Reader, Record, Value, Writer,
};
