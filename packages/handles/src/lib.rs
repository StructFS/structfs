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
//! - [`DuplexStream`]: a bounded, consuming byte transport with independent
//!   per-direction capacity and EOF state.
//! - [`Gate`] / [`CancelToken`]: park-until-predicate with the
//!   enable-before-check ordering baked in (no lost wakeups), and
//!   cancellation that fails parked reads while leaving writes open.
//! - [`conformance`]: certify any handle store against the protocol rules.
//!
//! ## Conventions
//!
//! **Constructors return `Self`.** Every constructor in this crate (and in
//! `structfs-service`, `structfs-state` and `structfs-profiles`) yields an
//! owned value; callers that need shared ownership wrap it in `Arc`
//! themselves. No constructor hides an `Arc` allocation.
//!
//! **Stop verbs.** `close()` requests cleanup and never blocks; where there
//! is cleanup to wait for, `join(timeout)` waits for it. [`DuplexStream`] has
//! only `close()`: its buffers are discarded synchronously.
//! [`HandleProtocol::close`] / [`HandleProtocol::close_wait`] /
//! [`HandleProtocol::close_complete`] are the protocol-level spellings of
//! the same three steps.

mod duplex;
pub use duplex::{DuplexStream, StreamReadiness, StreamStore};
mod gate;
mod handle_store;

pub mod conformance;

pub use gate::{CancelToken, Cancelled, Gate};
pub use handle_store::{HandleCx, HandleProtocol, HandleStore};

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

        fn write(
            &self,
            _handle: Arc<Self::Handle>,
            sub: Path,
            _data: Record,
        ) -> DetachedFuture<Path> {
            Box::pin(async move { Ok(sub) })
        }
    }

    #[tokio::test]
    async fn handle_store_passes_conformance() {
        let mut store = HandleStore::new(NullProtocol);
        conformance::check_handle_conventions(&mut store, Value::from("request")).await;
    }
}
