//! `ByteStream`: an append-only byte buffer with parked ranged reads.
//!
//! The byte-level analogue of [`crate::TailLog`], for stores that serve
//! `read(2)`-shaped traffic — stdio, sockets, file tails, response
//! bodies. The store convention it backs (see
//! `docs/patterns/bytestream.md`) serves ranges at `at/{offset}/len/{n}`
//! — the same path shape `structfs-sys` file handles use — with
//! `read(2)` semantics: a blocking read parks until at least one byte
//! past the offset exists, and returns empty exactly at end-of-stream.

use std::sync::Mutex;

use structfs_core_store::Value;

use crate::gate::{CancelToken, Cancelled, Gate};

/// One ranged read's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteChunk {
    /// The bytes read (possibly fewer than requested).
    pub bytes: Vec<u8>,
    /// The offset the bytes start at.
    pub offset: u64,
    /// True when this chunk ends at the end of a closed stream. For a
    /// parked read this is equivalent to `bytes.is_empty()` — the
    /// `read(2)` contract.
    pub eof: bool,
}

impl ByteChunk {
    /// The store-convention encoding: the bytes themselves. Empty bytes
    /// from a blocking read mean end-of-stream.
    pub fn into_value(self) -> Value {
        Value::Bytes(self.bytes)
    }

    /// The cursor for the next read.
    pub fn next_offset(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }
}

struct StreamState {
    data: Vec<u8>,
    closed: bool,
}

/// An append-only byte buffer with terminal state and parked ranged
/// reads.
pub struct ByteStream {
    state: Mutex<StreamState>,
    gate: Gate,
}

impl Default for ByteStream {
    fn default() -> Self {
        Self::new()
    }
}

impl ByteStream {
    /// Create an empty, open stream.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(StreamState {
                data: Vec::new(),
                closed: false,
            }),
            gate: Gate::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append bytes and wake parked readers.
    ///
    /// Returns false (dropping the bytes) if the stream is closed.
    pub fn push(&self, bytes: &[u8]) -> bool {
        {
            let mut state = self.lock();
            if state.closed {
                return false;
            }
            state.data.extend_from_slice(bytes);
        }
        self.gate.notify();
        true
    }

    /// Close the stream and wake parked readers. Idempotent.
    pub fn close(&self) {
        self.lock().closed = true;
        self.gate.notify();
    }

    /// Whether the stream is closed.
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Total bytes appended so far.
    pub fn len(&self) -> u64 {
        self.lock().data.len() as u64
    }

    /// Whether no bytes have been appended.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn chunk_at(state: &StreamState, offset: u64, max: usize) -> ByteChunk {
        // Clamp a past-the-end offset instead of erroring: it reads as
        // an empty chunk at the current end.
        let start = (offset as usize).min(state.data.len());
        let end = start.saturating_add(max).min(state.data.len());
        ByteChunk {
            bytes: state.data[start..end].to_vec(),
            offset: start as u64,
            eof: state.closed && end == state.data.len(),
        }
    }

    /// Non-blocking ranged read: whatever is available right now.
    pub fn snapshot_at(&self, offset: u64, max: usize) -> ByteChunk {
        Self::chunk_at(&self.lock(), offset, max)
    }

    /// `read(2)`: park until at least one byte past `offset` exists or
    /// the stream is closed. Returns an empty chunk exactly at
    /// end-of-stream.
    pub async fn read_at(&self, offset: u64, max: usize) -> ByteChunk {
        self.gate
            .wait_until(|| {
                let state = self.lock();
                if (state.data.len() as u64) > offset || state.closed {
                    Some(Self::chunk_at(&state, offset, max))
                } else {
                    None
                }
            })
            .await
    }

    /// [`ByteStream::read_at`], cancellable.
    pub async fn read_at_cancellable(
        &self,
        offset: u64,
        max: usize,
        token: &CancelToken,
    ) -> Result<ByteChunk, Cancelled> {
        self.gate
            .wait_until_cancellable(token, || {
                let state = self.lock();
                if (state.data.len() as u64) > offset || state.closed {
                    Some(Self::chunk_at(&state, offset, max))
                } else {
                    None
                }
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn ranged_reads_return_available_bytes() {
        let stream = ByteStream::new();
        stream.push(b"hello world");

        let chunk = stream.read_at(0, 5).await;
        assert_eq!(chunk.bytes, b"hello");
        assert_eq!(chunk.next_offset(), 5);
        assert!(!chunk.eof);

        let chunk = stream.read_at(chunk.next_offset(), 100).await;
        assert_eq!(chunk.bytes, b" world");
        assert!(!chunk.eof);
    }

    #[tokio::test]
    async fn read_parks_until_push() {
        let stream = Arc::new(ByteStream::new());
        let reader = {
            let stream = stream.clone();
            tokio::spawn(async move { stream.read_at(0, 16).await })
        };
        tokio::task::yield_now().await;
        stream.push(b"data");

        let chunk = reader.await.unwrap();
        assert_eq!(chunk.bytes, b"data");
    }

    #[tokio::test]
    async fn empty_read_means_eof() {
        let stream = Arc::new(ByteStream::new());
        stream.push(b"tail");

        // Reader drains, then parks; close resolves it with empty+eof.
        let reader = {
            let stream = stream.clone();
            tokio::spawn(async move {
                let first = stream.read_at(0, 100).await;
                let second = stream.read_at(first.next_offset(), 100).await;
                (first, second)
            })
        };
        tokio::task::yield_now().await;
        stream.close();

        let (first, second) = reader.await.unwrap();
        assert_eq!(first.bytes, b"tail");
        assert!(second.bytes.is_empty());
        assert!(second.eof);
    }

    #[tokio::test]
    async fn data_racing_close_arrives_before_eof() {
        // The close-out race, byte edition: bytes pushed just before
        // close must be readable before an empty EOF chunk is seen.
        let stream = Arc::new(ByteStream::new());
        let reader = {
            let stream = stream.clone();
            tokio::spawn(async move { stream.read_at(0, 100).await })
        };
        tokio::task::yield_now().await;
        stream.push(b"last words");
        stream.close();

        let chunk = reader.await.unwrap();
        assert_eq!(chunk.bytes, b"last words");
        assert!(chunk.eof);
    }

    #[tokio::test]
    async fn stale_offset_clamps() {
        let stream = ByteStream::new();
        stream.push(b"abc");
        stream.close();

        let chunk = stream.read_at(999, 10).await;
        assert!(chunk.bytes.is_empty());
        assert_eq!(chunk.offset, 3);
        assert!(chunk.eof);
    }

    #[test]
    fn push_after_close_is_dropped() {
        let stream = ByteStream::new();
        assert!(stream.push(b"a"));
        stream.close();
        assert!(!stream.push(b"b"));
        assert_eq!(stream.len(), 1);
    }

    #[test]
    fn snapshot_is_nonblocking() {
        let stream = ByteStream::new();
        let chunk = stream.snapshot_at(0, 10);
        assert!(chunk.bytes.is_empty());
        assert!(!chunk.eof); // open and empty: not EOF, just nothing yet
    }

    #[tokio::test]
    async fn cancellation_fails_parked_read() {
        let stream = Arc::new(ByteStream::new());
        let token = CancelToken::new();
        let reader = {
            let stream = stream.clone();
            let token = token.clone();
            tokio::spawn(async move { stream.read_at_cancellable(0, 10, &token).await })
        };
        tokio::task::yield_now().await;
        token.cancel();
        assert!(reader.await.unwrap().is_err());
    }

    #[test]
    fn value_encoding_is_bytes() {
        let chunk = ByteChunk {
            bytes: vec![1, 2, 3],
            offset: 0,
            eof: false,
        };
        assert_eq!(chunk.into_value(), Value::Bytes(vec![1, 2, 3]));
    }
}
