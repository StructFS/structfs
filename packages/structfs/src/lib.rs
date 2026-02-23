//! StructFS: A uniform interface for accessing data through read/write operations on paths.
//!
//! StructFS provides a "everything is a store" abstraction where all data access — including
//! mount management, HTTP requests, and configuration — happens through the same read/write
//! interface on paths.
