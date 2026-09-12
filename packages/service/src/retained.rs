//! Bounded, owned byte results and event tails. Copies returned to consumers are
//! consumer-owned; reserve their transport/response budget separately.
use crate::{CancelToken, Error, OwnerHandle, Registration, ResourceKind};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use structfs_handles::Gate;

/// A result whose storage is registered before it can be delivered. Abandoning
/// delivery drops this handle and schedules cleanup; owner cancellation also
/// releases storage even if callers keep the handle.
pub struct RetainedBytes {
    inner: Arc<Mutex<Option<Vec<u8>>>>,
    registration: Registration,
}
impl RetainedBytes {
    pub fn new(owner: &OwnerHandle, value: Vec<u8>) -> Result<Self, Error> {
        let bytes = value.capacity();
        let inner = Arc::new(Mutex::new(Some(value)));
        let cleanup = inner.clone();
        let registration = owner.register(ResourceKind::Retained, bytes, move || async move {
            *cleanup.lock().unwrap_or_else(|e| e.into_inner()) = None;
            Ok(())
        })?;
        Ok(Self {
            inner,
            registration,
        })
    }
    pub fn snapshot(&self) -> Result<Vec<u8>, Error> {
        if self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("result released"));
        }
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| Error::cancelled("result released"))
    }
    pub fn release(&self) {
        self.registration.release();
    }
}
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
#[derive(Debug, PartialEq, Eq)]
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
            return Err(Error::overloaded("tail item capacity must be positive"));
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
        if self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("tail released"));
        }
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.done {
            return Err(Error::cancelled("tail finished"));
        }
        if s.items.len() >= self.max_items
            || value.capacity() > self.max_bytes.saturating_sub(s.bytes)
        {
            return Err(Error::overloaded("tail capacity exhausted"));
        }
        let next = s
            .next
            .checked_add(1)
            .ok_or_else(|| Error::overloaded("tail cursor exhausted"))?;
        let seq = s.next;
        s.next = next;
        s.bytes += value.capacity();
        s.items.push_back(value);
        drop(s);
        self.inner.gate.notify();
        Ok(seq)
    }
    pub fn finish(&self) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .done = true;
        self.inner.gate.notify();
    }
    pub fn release(&self) {
        self.registration.release();
    }
    /// Discard entries strictly before cursor. Cursor must be in the retained range.
    pub fn acknowledge(&self, cursor: u64) -> Result<(), Error> {
        let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.released {
            return Err(Error::cancelled("tail released"));
        }
        if cursor < s.first || cursor > s.next {
            return Err(Error::conflict("tail cursor outside retained range"));
        }
        while s.first < cursor {
            let v = s.items.pop_front().unwrap();
            s.bytes -= v.capacity();
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
        if max_items == 0 {
            return Err(Error::conflict("tail page must contain at least one item"));
        }
        let revoked = self.registration.cancellation();
        let read = self.inner.gate.wait_until_cancellable(cancel, || {
            let s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.released {
                return Some(Err(Error::cancelled("tail released")));
            }
            if cursor < s.first || cursor > s.next {
                return Some(Err(Error::conflict("tail cursor outside retained range")));
            }
            if cursor == s.next && !s.done {
                return None;
            }
            let items: Vec<_> = s
                .items
                .iter()
                .skip((cursor - s.first) as usize)
                .take(max_items)
                .cloned()
                .collect();
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
