//! # structfs-handles
//!
//! Handle-store and streaming primitives for StructFS.
//!
//! The deferred-operation pattern — write a request, get an
//! `outstanding/{id}` handle path back, read the handle for results — is
//! the backbone of every broker-shaped StructFS store. This crate makes it
//! a primitive instead of a convention:
//!
//! - [`HandleStore`] + [`HandleProtocol`]: generic `outstanding/{id}`
//!   scaffolding — id minting, routing, the no-overwrite rule, Null-write
//!   release with cancellation, listing.
//! - [`TailLog`] / [`TailPage`]: append-only event streams with **atomic
//!   tail reads** — items and terminal status in one operation, so the
//!   "close-out drain" race cannot exist.
//! - [`Gate`] / [`CancelToken`]: park-until-predicate with the
//!   enable-before-check ordering baked in (no lost wakeups), and
//!   cancellation that fails parked reads while leaving writes open.
//! - [`SyncBridge`]: run a detached async store from synchronous code on a
//!   blocking thread.
//! - [`conformance`]: certify any handle store against the protocol rules.

mod gate;
mod handle_store;
mod sync_bridge;
mod tail;

pub mod conformance;

pub use gate::{CancelToken, Cancelled, Gate};
pub use handle_store::{HandleCx, HandleProtocol, HandleStore};
pub use sync_bridge::SyncBridge;
pub use tail::{TailLog, TailPage};

// Re-export the async trait surface these types implement.
pub use structfs_core_store::{
    DetachedFuture, DetachedReader, DetachedStore, DetachedWriter, Error, Path, Record, Value,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NullProtocol;

    impl HandleProtocol for NullProtocol {
        type Handle = Value;

        fn open(&self, _cx: HandleCx, request: Value) -> Result<Self::Handle, Error> {
            Ok(request)
        }

        fn read(&self, handle: Arc<Self::Handle>, _sub: Path) -> DetachedFuture<Option<Record>> {
            Box::pin(async move { Ok(Some(Record::parsed((*handle).clone()))) })
        }

        fn write(&self, _handle: Arc<Self::Handle>, sub: Path, _data: Record) -> DetachedFuture<Path> {
            Box::pin(async move { Ok(sub) })
        }
    }

    #[tokio::test]
    async fn handle_store_passes_conformance() {
        let mut store = HandleStore::new(NullProtocol);
        conformance::check_handle_conventions(&mut store, Value::from("request")).await;
    }
}
