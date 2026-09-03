//! `TailLog`: an append-only event log with atomic tail reads.
//!
//! Streaming stores conventionally serve events at `events/from/{seq}` and
//! terminal state at a separate path. Because those are two reads, every
//! consumer needs a "close-out drain" for events landing between the tail
//! read and the terminal check. `TailLog::read_from` returns items *and*
//! terminal status in one atomic operation, so that race cannot exist.

use std::sync::Mutex;

use structfs_core_store::Value;

use crate::gate::{CancelToken, Cancelled, Gate};

/// One page of a tail read.
#[derive(Debug, Clone, PartialEq)]
pub struct TailPage<T> {
    /// Events from the requested cursor to the end of the log.
    pub items: Vec<T>,
    /// The cursor to pass to the next read.
    pub next: u64,
    /// Whether the log is finished. When true, no more events will ever
    /// arrive; the consumer can stop without a close-out drain.
    pub done: bool,
}

impl<T: Into<Value>> TailPage<T> {
    /// Encode as the conventional tail envelope:
    /// `{items: [...], next: N, status: "open" | "done"}`.
    pub fn into_value(self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "items".to_string(),
            Value::Array(self.items.into_iter().map(Into::into).collect()),
        );
        map.insert("next".to_string(), Value::Integer(self.next as i64));
        map.insert(
            "status".to_string(),
            Value::String(if self.done { "done" } else { "open" }.to_string()),
        );
        Value::Map(map)
    }
}

struct TailState<T> {
    events: Vec<T>,
    done: bool,
}

/// An append-only event log with terminal state and parked tail reads.
pub struct TailLog<T> {
    state: Mutex<TailState<T>>,
    gate: Gate,
}

impl<T: Clone> Default for TailLog<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> TailLog<T> {
    /// Create an empty, open log.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TailState {
                events: Vec::new(),
                done: false,
            }),
            gate: Gate::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TailState<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append an event and wake parked readers.
    ///
    /// Returns false (dropping the event) if the log is already finished.
    pub fn push(&self, event: T) -> bool {
        {
            let mut state = self.lock();
            if state.done {
                return false;
            }
            state.events.push(event);
        }
        self.gate.notify();
        true
    }

    /// Mark the log finished and wake parked readers. Idempotent.
    pub fn finish(&self) {
        self.lock().done = true;
        self.gate.notify();
    }

    /// Whether the log is finished.
    pub fn is_done(&self) -> bool {
        self.lock().done
    }

    /// Number of events appended so far.
    pub fn len(&self) -> u64 {
        self.lock().events.len() as u64
    }

    /// Whether no events have been appended.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn page_from(state: &TailState<T>, seq: u64) -> TailPage<T> {
        // Clamp an out-of-range cursor instead of panicking: a stale or
        // corrupt cursor yields an empty page at the log's end.
        let start = (seq as usize).min(state.events.len());
        TailPage {
            items: state.events[start..].to_vec(),
            next: state.events.len() as u64,
            done: state.done,
        }
    }

    /// Non-blocking snapshot of the tail from `seq`.
    pub fn snapshot_from(&self, seq: u64) -> TailPage<T> {
        Self::page_from(&self.lock(), seq)
    }

    /// Atomic tail read: park until there are events past `seq` or the log
    /// is finished, then return them together with the terminal status.
    pub async fn read_from(&self, seq: u64) -> TailPage<T> {
        self.gate
            .wait_until(|| {
                let state = self.lock();
                if (state.events.len() as u64) > seq || state.done {
                    Some(Self::page_from(&state, seq))
                } else {
                    None
                }
            })
            .await
    }

    /// [`TailLog::read_from`], cancellable.
    pub async fn read_from_cancellable(
        &self,
        seq: u64,
        token: &CancelToken,
    ) -> Result<TailPage<T>, Cancelled> {
        self.gate
            .wait_until_cancellable(token, || {
                let state = self.lock();
                if (state.events.len() as u64) > seq || state.done {
                    Some(Self::page_from(&state, seq))
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
    async fn tail_read_returns_existing_events() {
        let log = TailLog::new();
        log.push(1);
        log.push(2);

        let page = log.read_from(0).await;
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(page.next, 2);
        assert!(!page.done);
    }

    #[tokio::test]
    async fn tail_read_parks_until_push() {
        let log = Arc::new(TailLog::new());
        let reader = {
            let log = log.clone();
            tokio::spawn(async move { log.read_from(0).await })
        };
        tokio::task::yield_now().await;
        log.push("event");

        let page = reader.await.unwrap();
        assert_eq!(page.items, vec!["event"]);
    }

    #[tokio::test]
    async fn terminal_status_arrives_with_items() {
        // The close-out race: events pushed immediately before finish must
        // arrive in the same page that reports done.
        let log = Arc::new(TailLog::new());
        log.push(1);

        let reader = {
            let log = log.clone();
            tokio::spawn(async move {
                let first = log.read_from(0).await;
                let second = log.read_from(first.next).await;
                (first, second)
            })
        };
        tokio::task::yield_now().await;
        log.push(2);
        log.finish();

        let (first, second) = reader.await.unwrap();
        assert_eq!(first.items, vec![1]);
        // Second read gets the racing event AND the terminal flag together.
        assert_eq!(second.items, vec![2]);
        assert!(second.done);
    }

    #[tokio::test]
    async fn finish_unblocks_empty_tail() {
        let log: Arc<TailLog<i32>> = Arc::new(TailLog::new());
        let reader = {
            let log = log.clone();
            tokio::spawn(async move { log.read_from(0).await })
        };
        tokio::task::yield_now().await;
        log.finish();

        let page = reader.await.unwrap();
        assert!(page.items.is_empty());
        assert!(page.done);
    }

    #[tokio::test]
    async fn stale_cursor_clamps() {
        let log = TailLog::new();
        log.push(1);
        log.finish();

        let page = log.read_from(999).await;
        assert!(page.items.is_empty());
        assert_eq!(page.next, 1);
        assert!(page.done);
    }

    #[test]
    fn push_after_finish_is_dropped() {
        let log = TailLog::new();
        assert!(log.push(1));
        log.finish();
        assert!(!log.push(2));
        assert_eq!(log.len(), 1);
    }

    #[tokio::test]
    async fn cancellation_fails_parked_read() {
        let log: Arc<TailLog<i32>> = Arc::new(TailLog::new());
        let token = CancelToken::new();
        let reader = {
            let log = log.clone();
            let token = token.clone();
            tokio::spawn(async move { log.read_from_cancellable(0, &token).await })
        };
        tokio::task::yield_now().await;
        token.cancel();

        assert!(reader.await.unwrap().is_err());
    }

    #[test]
    fn page_value_encoding() {
        let page = TailPage {
            items: vec![Value::from(1i64)],
            next: 1,
            done: true,
        };
        let value = page.into_value();
        let map = match value {
            Value::Map(m) => m,
            _ => panic!("expected map"),
        };
        assert_eq!(map.get("next"), Some(&Value::Integer(1)));
        assert_eq!(map.get("status"), Some(&Value::from("done")));
        assert!(matches!(map.get("items"), Some(Value::Array(a)) if a.len() == 1));
    }
}
