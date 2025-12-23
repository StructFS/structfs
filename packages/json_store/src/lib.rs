pub mod in_memory;
pub mod json_utils;
pub mod local_disk;

pub use structfs_store::{Path, PathError};
pub use structfs_store::store;

pub use in_memory::SerdeJSONInMemoryStore;
pub use local_disk::JSONLocalStore;
