//! Bounded, owned event tails. Copies returned to consumers are
//! consumer-owned; reserve their transport/response budget separately.
use crate::{CancelToken, Error, OwnerHandle, Registration, ResourceKind};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use structfs_handles::Gate;

struct TailState {
    items: VecDeque<Vec<u8>>,
    first: u64,
    next: u64,
    bytes: usize,
    done: bool,
    released: bool,
}
struct TailInner {
    state: Mutex<TailState>,
    gate: Gate,
}
/// One page of a tail read: the items from the requested cursor, the cursor to
/// pass next, and whether the tail is finished.
///
/// This is the one page envelope across `structfs-service` and
/// `structfs-state`: `SnapshotPage` and `ChangePage` carry the same
/// `items`/`next`/`done` shape with a typed cursor.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TailRead {
    pub items: Vec<Vec<u8>>,
    pub next: u64,
    pub done: bool,
}
/// Bounded event tail with explicit acknowledgement. Full tails reject writes;
/// there is no silent eviction. Stale/future cursors fail. Finish preserves the
/// readable tail; release/owner close clears it and wakes parked readers.
pub struct OwnedTail {
    inner: Arc<TailInner>,
    registration: Registration,
    max_items: usize,
    max_bytes: usize,
}
impl OwnedTail {
    pub fn new(owner: &OwnerHandle, max_items: usize, max_bytes: usize) -> Result<Self, Error> {
        if max_items == 0 {
            return Err(Error::invalid_argument(
                "tail item capacity must be positive",
            ));
        }
        let reserved = max_items
            .checked_mul(std::mem::size_of::<Vec<u8>>())
            .and_then(|n| n.checked_add(max_bytes))
            .ok_or_else(|| Error::overloaded("tail capacity overflow"))?;
        let inner = Arc::new(TailInner {
            state: Mutex::new(TailState {
                items: VecDeque::new(),
                first: 0,
                next: 0,
                bytes: 0,
                done: false,
                released: false,
            }),
            gate: Gate::new(),
        });
        let cleanup = inner.clone();
        let registration =
            owner.register(ResourceKind::Retained, reserved, move || async move {
                let mut s = cleanup.state.lock().unwrap_or_else(|e| e.into_inner());
                s.items = VecDeque::new();
                s.bytes = 0;
                s.done = true;
                s.released = true;
                drop(s);
                cleanup.gate.notify();
                Ok(())
            })?;
        Ok(Self {
            inner,
            registration,
            max_items,
            max_bytes,
        })
    }
    pub fn push(&self, value: Vec<u8>) -> Result<u64, Error> {
        self.push_batch(vec![value]).map(|range| range.start)
    }
    /// Accept all items atomically or none. Capacity counts retained allocations;
    /// rejection reaches the producer before any cursor or item is published.
    pub fn push_batch(&self, values: Vec<Vec<u8>>) -> Result<std::ops::Range<u64>, Error> {
        if self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("tail released"));
        }
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.done {
            return Err(Error::cancelled("tail finished"));
        }
        // Charge the same measure everywhere: `read_bounded` pages by
        // `len()`, `acknowledge` refunds by `len()`, so admission must count
        // `len()` too. Charging `capacity()` here and refunding `len()` let
        // the accounting drift upward on every over-allocated Vec.
        let bytes = values
            .iter()
            .try_fold(0usize, |n, v| n.checked_add(v.len()))
            .ok_or_else(|| Error::overloaded("tail byte count overflow"))?;
        if values.len() > self.max_items.saturating_sub(s.items.len())
            || bytes > self.max_bytes.saturating_sub(s.bytes)
        {
            return Err(Error::overloaded("tail capacity exhausted"));
        }
        let next = s
            .next
            .checked_add(values.len() as u64)
            .ok_or_else(|| Error::overloaded("tail cursor exhausted"))?;
        let first = s.next;
        s.next = next;
        s.bytes += bytes;
        s.items.extend(values);
        drop(s);
        self.inner.gate.notify();
        Ok(first..next)
    }
    pub fn finish(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .done = true;
        self.inner.gate.notify();
    }
    /// Request release of the tail's storage. Non-blocking; parked readers
    /// wake and fail, and further pushes are refused.
    pub fn close(&self) {
        self.registration.close();
    }
    /// [`OwnedTail::close`], then wait at most `timeout` for the storage to be
    /// released, and report the owner's remaining resources.
    pub async fn join(&self, timeout: std::time::Duration) -> crate::CloseReport {
        self.registration.join(timeout).await
    }
    /// Discard entries strictly before cursor. Cursor must be in the retained range.
    pub fn acknowledge(&self, cursor: u64) -> Result<(), Error> {
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.released {
            return Err(Error::cancelled("tail released"));
        }
        if cursor < s.first || cursor > s.next {
            return Err(Error::invalid_argument(
                "tail cursor outside retained range",
            ));
        }
        while s.first < cursor {
            let v = s.items.pop_front().unwrap();
            s.bytes -= v.len();
            s.first += 1;
        }
        Ok(())
    }
    pub async fn read(
        &self,
        cursor: u64,
        max_items: usize,
        cancel: &CancelToken,
    ) -> Result<TailRead, Error> {
        self.read_bounded(cursor, max_items, usize::MAX, cancel)
            .await
    }
    /// Bound both returned item count and payload bytes before cloning. If the
    /// first item cannot fit, fail rather than return a non-advancing empty page.
    /// Encoding overhead remains the transport's separate responsibility.
    pub async fn read_bounded(
        &self,
        cursor: u64,
        max_items: usize,
        max_bytes: usize,
        cancel: &CancelToken,
    ) -> Result<TailRead, Error> {
        if max_items == 0 {
            return Err(Error::invalid_argument(
                "tail page must contain at least one item",
            ));
        }
        let revoked = self.registration.cancellation();
        let read = self.inner.gate.wait_until_cancellable(cancel, || {
            let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.released {
                return Some(Err(Error::cancelled("tail released")));
            }
            if cursor < s.first || cursor > s.next {
                return Some(Err(Error::invalid_argument(
                    "tail cursor outside retained range",
                )));
            }
            if cursor == s.next && !s.done {
                return None;
            }
            let mut items = Vec::new();
            let mut bytes = 0usize;
            for item in s
                .items
                .iter()
                .skip((cursor - s.first) as usize)
                .take(max_items)
            {
                if item.len() > max_bytes.saturating_sub(bytes) {
                    if items.is_empty() {
                        return Some(Err(Error::resource_limit(
                            "tail item exceeds page byte limit",
                        )));
                    }
                    break;
                }
                bytes += item.len();
                items.push(item.clone());
            }
            let next = cursor + items.len() as u64;
            Some(Ok(TailRead {
                items,
                next,
                done: s.done && next == s.next,
            }))
        });
        tokio::select! { biased;
            _ = revoked.cancelled() => Err(Error::cancelled("tail released")),
            result = read => result.map_err(|e| e.into_error("tail read"))?,
        }
    }
}
