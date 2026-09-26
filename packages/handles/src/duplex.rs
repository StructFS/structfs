//! Bounded, consuming byte streams: consumed bytes are reclaimed, and each
//! direction has an independent capacity and EOF state.
use crate::{
    CancelToken, DetachedFuture, DetachedReader, DetachedWriter, Error, Gate, Path, Record, Value,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

struct State {
    bytes: VecDeque<u8>,
    closed: bool,
    released: bool,
}
struct Pipe {
    capacity: usize,
    state: Mutex<State>,
    gate: Gate,
}
impl Pipe {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            state: Mutex::new(State {
                bytes: VecDeque::new(),
                closed: false,
                released: false,
            }),
            gate: Gate::new(),
        })
    }
    fn close(&self, release: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        if release {
            state.released = true;
            state.bytes.clear();
        }
        drop(state);
        self.gate.notify();
    }
    async fn read(&self, max: usize, cancel: &CancelToken) -> Result<Vec<u8>, Error> {
        if max == 0 {
            return Err(Error::invalid_argument("read size must be positive"));
        }
        let result = self
            .gate
            .wait_until_cancellable(cancel, || {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.released {
                    return Some(Err(Error::cancelled("stream released")));
                }
                if !state.bytes.is_empty() || state.closed {
                    let count = max.min(state.bytes.len());
                    return Some(Ok(state.bytes.drain(..count).collect()));
                }
                None
            })
            .await
            .map_err(|e| e.into_error("stream read cancelled"))?;
        self.gate.notify();
        result
    }
    async fn write(&self, bytes: &[u8], cancel: &CancelToken) -> Result<(), Error> {
        if bytes.len() > self.capacity {
            return Err(Error::resource_limit(
                "write exceeds stream capacity; chunk it",
            ));
        }
        let result = self
            .gate
            .wait_until_cancellable(cancel, || {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.closed {
                    return Some(Err(Error::cancelled("stream write side closed")));
                }
                if bytes.len() <= self.capacity - state.bytes.len() {
                    state.bytes.extend(bytes);
                    return Some(Ok(()));
                }
                None
            })
            .await
            .map_err(|e| e.into_error("stream write cancelled"))?;
        self.gate.notify();
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamReadiness {
    pub readable_bytes: usize,
    pub writable_bytes: usize,
    pub eof: bool,
    pub write_closed: bool,
    /// Set once either end has been closed outright (rather than half-closed).
    pub closed: bool,
}
/// An endpoint owns both half-streams. Share it with Arc; dropping the last
/// owner closes the stream, discarding buffers and waking both ends. A
/// cancelled operation transfers no bytes; a successful write transfers the
/// entire chunk atomically.
pub struct DuplexStream {
    rx: Arc<Pipe>,
    tx: Arc<Pipe>,
}
impl DuplexStream {
    /// Two connected endpoints, each with `capacity_per_direction` bytes of
    /// buffer in each direction. Wrap them in `Arc` to share an endpoint.
    pub fn pair(capacity_per_direction: usize) -> Result<(Self, Self), Error> {
        if capacity_per_direction == 0 {
            return Err(Error::invalid_argument("stream capacity must be positive"));
        }
        let a = Pipe::new(capacity_per_direction);
        let b = Pipe::new(capacity_per_direction);
        Ok((
            Self {
                rx: a.clone(),
                tx: b.clone(),
            },
            Self { rx: b, tx: a },
        ))
    }
    pub async fn read(&self, max: usize, cancel: &CancelToken) -> Result<Vec<u8>, Error> {
        self.rx.read(max, cancel).await
    }
    pub async fn write(&self, bytes: &[u8], cancel: &CancelToken) -> Result<(), Error> {
        self.tx.write(bytes, cancel).await
    }
    /// Half-close: the peer drains queued bytes, then reads EOF. Receive stays open.
    pub fn shutdown_write(&self) {
        self.tx.close(false);
    }
    /// Request teardown: discard queues and fail all parked operations, at
    /// both ends. Non-blocking and idempotent; there is nothing left to join.
    pub fn close(&self) {
        self.rx.close(true);
        self.tx.close(true);
    }
    /// Advisory snapshot; another consumer may change readiness before an operation.
    pub fn readiness(&self) -> StreamReadiness {
        // Never hold both mutexes: the peer observes these pipes in reverse order.
        let (readable_bytes, eof, closed) = {
            let state = self.rx.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.bytes.len(),
                state.closed && state.bytes.is_empty(),
                state.released,
            )
        };
        let state = self.tx.state.lock().unwrap_or_else(|e| e.into_inner());
        StreamReadiness {
            readable_bytes,
            eof,
            closed: closed || state.released,
            write_closed: state.closed,
            writable_bytes: if state.closed {
                0
            } else {
                self.tx.capacity - state.bytes.len()
            },
        }
    }
    /// Wait for receive data/EOF or transmit capacity. Specify only interests
    /// needed by the caller; writable sockets usually become ready immediately.
    pub async fn ready(
        &self,
        read: bool,
        write: bool,
        cancel: &CancelToken,
    ) -> Result<StreamReadiness, Error> {
        if !read && !write {
            return Err(Error::invalid_argument("readiness needs an interest"));
        }
        let check = || {
            let r = self.readiness();
            (r.closed
                || (read && (r.readable_bytes > 0 || r.eof))
                || (write && (r.writable_bytes > 0 || r.write_closed)))
                .then_some(r)
        };
        tokio::select! {
            r = self.rx.gate.wait_until_cancellable(cancel, check) => r,
            r = self.tx.gate.wait_until_cancellable(cancel, check) => r,
        }
        .map_err(|e| e.into_error("stream readiness cancelled"))
    }
    /// Mount this endpoint through the standard consuming stream protocol.
    pub fn store(self: &Arc<Self>) -> StreamStore {
        StreamStore(self.clone())
    }
}
impl Drop for DuplexStream {
    fn drop(&mut self) {
        self.close();
    }
}

/// Paths relative to an explicitly granted stream handle:
/// read `rx/{max}` -> Bytes (empty means EOF), write `tx` -> Bytes;
/// read `ready/{read|write|both}` parks; write Null to `shutdown` half-closes,
/// write Null to the root closes. No transport creates ambient network authority.
#[derive(Clone)]
pub struct StreamStore(Arc<DuplexStream>);
impl DetachedReader for StreamStore {
    fn read_detached(&mut self, path: &Path) -> DetachedFuture<Option<Record>> {
        let stream = self.0.clone();
        let path = path.clone();
        Box::pin(async move {
            let parts: Vec<_> = path.iter().collect();
            let cancel = CancelToken::new();
            let value = match parts.as_slice() {
                ["rx", max] => Value::Bytes(
                    stream
                        .read(
                            max.parse()
                                .map_err(|_| Error::invalid_argument("invalid read size"))?,
                            &cancel,
                        )
                        .await?,
                ),
                ["ready", interest @ ("read" | "write" | "both")] => {
                    let r = stream
                        .ready(*interest != "write", *interest != "read", &cancel)
                        .await?;
                    Value::Map(std::collections::BTreeMap::from([
                        (
                            "readable_bytes".into(),
                            Value::Integer(r.readable_bytes as i64),
                        ),
                        (
                            "writable_bytes".into(),
                            Value::Integer(r.writable_bytes as i64),
                        ),
                        ("eof".into(), Value::Bool(r.eof)),
                        ("write_closed".into(), Value::Bool(r.write_closed)),
                        ("closed".into(), Value::Bool(r.closed)),
                    ]))
                }
                _ => return Err(Error::not_found(path.clone())),
            };
            Ok(Some(Record::parsed(value)))
        })
    }
}
impl DetachedWriter for StreamStore {
    fn write_detached(&mut self, path: &Path, data: Record) -> DetachedFuture<Path> {
        let stream = self.0.clone();
        let path = path.clone();
        Box::pin(async move {
            let value = data.into_value(&structfs_core_store::NoCodec)?;
            if path.is_empty() && value.is_null() {
                stream.close();
            } else if path.to_string() == "shutdown" && value.is_null() {
                stream.shutdown_write();
            } else if path.to_string() == "tx" {
                let Value::Bytes(bytes) = value else {
                    return Err(Error::invalid_argument("tx expects Bytes"));
                };
                stream.write(&bytes, &CancelToken::new()).await?;
            } else {
                return Err(Error::not_found(path.clone()));
            }
            Ok(path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert a future has not resolved, by polling it exactly once. Unlike a
    /// short `timeout`, this cannot pass or fail on scheduler luck.
    async fn assert_pending<F: std::future::Future + Unpin>(future: &mut F) {
        std::future::poll_fn(|cx| {
            assert!(std::pin::Pin::new(&mut *future).poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn backpressure_half_close_and_reverse_traffic() {
        let (a, b) = DuplexStream::pair(4).unwrap();
        let cancel = CancelToken::new();
        a.write(b"abcd", &cancel).await.unwrap();
        assert_eq!(a.readiness().writable_bytes, 0);
        assert!(a.write(b"large", &cancel).await.is_err());
        let write = a.write(b"ef", &cancel);
        tokio::pin!(write);
        assert_pending(&mut write).await;
        assert_eq!(b.read(2, &cancel).await.unwrap(), b"ab");
        write.await.unwrap();
        a.shutdown_write();
        assert_eq!(b.read(4, &cancel).await.unwrap(), b"cdef");
        assert!(b.read(1, &cancel).await.unwrap().is_empty());
        b.write(b"back", &cancel).await.unwrap();
        assert_eq!(a.read(4, &cancel).await.unwrap(), b"back");
    }
    #[tokio::test]
    async fn cancel_and_close_wake_without_transferring_bytes() {
        let (a, b) = DuplexStream::pair(1).unwrap();
        let cancel = CancelToken::new();
        a.write(b"a", &cancel).await.unwrap();
        cancel.cancel();
        assert!(a.write(b"b", &cancel).await.is_err());
        assert_eq!(b.read(1, &CancelToken::new()).await.unwrap(), b"a");
        let token = CancelToken::new();
        let waiting = b.ready(true, false, &token);
        // Drop closes the endpoint, including waking a peer with no data.
        drop(a);
        assert!(waiting.await.unwrap().closed);
        assert!(b.read(1, &CancelToken::new()).await.is_err());
    }

    #[tokio::test]
    async fn zero_sized_arguments_are_invalid_not_capacity_failures() {
        assert!(matches!(
            DuplexStream::pair(0),
            Err(Error::InvalidArgument { .. })
        ));
        let (a, _b) = DuplexStream::pair(1).unwrap();
        let cancel = CancelToken::new();
        assert!(matches!(
            a.read(0, &cancel).await,
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            a.ready(false, false, &cancel).await,
            Err(Error::InvalidArgument { .. })
        ));
        // Capacity failures stay capacity failures.
        assert!(matches!(
            a.write(b"too long", &cancel).await,
            Err(Error::ResourceLimit { .. })
        ));
    }
}
