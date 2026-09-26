//! Typed operations with no retained store, path, or input borrow.
//!
//! These mirror the synchronous [`TypedReader`](crate::TypedReader) /
//! [`TypedWriter`](crate::TypedWriter) surface; see the
//! [crate docs](crate#typed-access) for the matrix and the codec convention.
use crate::{from_value, to_value};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use structfs_core_store::{Codec, DetachedFuture, DetachedReader, DetachedWriter, Path, Record};

/// Typed reads that can remain parked while other store operations proceed.
/// Parsed records are converted directly; raw records require the supplied
/// codec. Codec format, validation, and typed conversion errors are preserved.
/// The owned codec handle lives until decoding completes.
///
/// There is no `read_children_typed_detached`: [`DetachedReader`] has no
/// child enumeration to build it on.
pub trait DetachedTypedReader: DetachedReader {
    /// Read and deserialize, parsing raw records with `codec`.
    fn read_as_detached<T: DeserializeOwned + Send + 'static>(
        &mut self,
        from: &Path,
        codec: Arc<dyn Codec>,
    ) -> DetachedFuture<Option<T>> {
        let operation = self.read_detached(from);
        Box::pin(async move {
            operation
                .await?
                .map(|record| from_value(record.into_value(codec.as_ref())?))
                .transpose()
        })
    }

    /// Codec-free access to parsed records; raw records fail with NoCodec's error.
    fn read_typed_detached<T: DeserializeOwned + Send + 'static>(
        &mut self,
        from: &Path,
    ) -> DetachedFuture<Option<T>> {
        self.read_as_detached(from, Arc::new(structfs_core_store::NoCodec))
    }
}
impl<R: DetachedReader + ?Sized> DetachedTypedReader for R {}

/// Serialization completes before constructing the underlying write. A failed
/// conversion constructs no store operation. The returned future borrows neither
/// the input nor the store; creating a successful operation may have effects.
pub trait DetachedTypedWriter: DetachedWriter {
    /// Serialize and write. As in the other flavours there is no codec-taking
    /// counterpart: a typed write always produces a parsed record.
    fn write_typed_detached<T: Serialize + ?Sized>(
        &mut self,
        to: &Path,
        data: &T,
    ) -> DetachedFuture<Path> {
        match to_value(data) {
            Ok(value) => self.write_detached(to, Record::parsed(value)),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}
impl<W: DetachedWriter + ?Sized> DetachedTypedWriter for W {}
