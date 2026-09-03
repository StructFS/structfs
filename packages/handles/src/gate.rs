//! Parking primitives: `Gate` and `CancelToken`.
//!
//! Every hand-rolled handle store repeats the same subtle `Notify` dance:
//! the notified future must be created and enabled *before* the state
//! check, or a notification landing between the check and the await is
//! lost and the reader parks forever. `Gate` owns that ordering so store
//! authors never write it again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// Park until a predicate holds.
///
/// `wait_until` re-runs `check` each time the gate is notified and resolves
/// with the first `Some` it produces. The enable-before-check ordering is
/// internal, so a `notify` racing the check is never lost.
#[derive(Default)]
pub struct Gate {
    notify: Notify,
}

impl Gate {
    /// Create a gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wake all parked waiters so they re-run their predicates.
    ///
    /// Call after every state change a waiter might be watching.
    pub fn notify(&self) {
        self.notify.notify_waiters();
    }

    /// Park until `check` returns `Some`.
    pub async fn wait_until<T>(&self, mut check: impl FnMut() -> Option<T>) -> T {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Enable the future BEFORE checking, so a notify between the
            // check and the await still wakes us.
            notified.as_mut().enable();
            if let Some(value) = check() {
                return value;
            }
            notified.await;
        }
    }

    /// Park until `check` returns `Some` or the token is cancelled.
    pub async fn wait_until_cancellable<T>(
        &self,
        token: &CancelToken,
        mut check: impl FnMut() -> Option<T>,
    ) -> Result<T, Cancelled> {
        loop {
            let notified = self.notify.notified();
            let cancelled = token.inner.notify.notified();
            tokio::pin!(notified);
            tokio::pin!(cancelled);
            notified.as_mut().enable();
            cancelled.as_mut().enable();

            if token.is_cancelled() {
                return Err(Cancelled);
            }
            if let Some(value) = check() {
                return Ok(value);
            }
            tokio::select! {
                _ = notified => {}
                _ = cancelled => {}
            }
        }
    }
}

/// The wait was cancelled via its [`CancelToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl Cancelled {
    /// Convert into a store error with context.
    pub fn into_error(self, context: &str) -> structfs_core_store::Error {
        structfs_core_store::Error::cancelled(context.to_string())
    }
}

#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
}

/// A cloneable cancellation token.
///
/// Cancelling wakes every parked [`Gate::wait_until_cancellable`] carrying
/// the token. By the handle-store protocol rule, cancellation fails parked
/// *reads*; writes are not cancelled, so teardown writes can still land.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

impl CancelToken {
    /// Create an un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel: wake all waiters. Idempotent.
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// Whether the token has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }

    /// Park until the token is cancelled.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn wait_resolves_when_predicate_holds() {
        let gate = Arc::new(Gate::new());
        let slot: Arc<Mutex<Option<i32>>> = Arc::new(Mutex::new(None));

        let waiter = {
            let gate = gate.clone();
            let slot = slot.clone();
            tokio::spawn(async move { gate.wait_until(|| *slot.lock().unwrap()).await })
        };

        tokio::task::yield_now().await;
        *slot.lock().unwrap() = Some(7);
        gate.notify();

        assert_eq!(waiter.await.unwrap(), 7);
    }

    #[tokio::test]
    async fn notify_racing_check_is_not_lost() {
        // Hammer the race: a notifier flips state and notifies while the
        // waiter is between its check and its await. With enable-before-
        // check this always resolves; without it, it can hang.
        for _ in 0..100 {
            let gate = Arc::new(Gate::new());
            let flag = Arc::new(AtomicBool::new(false));

            let waiter = {
                let gate = gate.clone();
                let flag = flag.clone();
                tokio::spawn(async move {
                    gate.wait_until(|| flag.load(Ordering::SeqCst).then_some(()))
                        .await
                })
            };
            let notifier = {
                let gate = gate.clone();
                let flag = flag.clone();
                tokio::spawn(async move {
                    flag.store(true, Ordering::SeqCst);
                    gate.notify();
                })
            };

            tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
                .await
                .expect("lost wakeup")
                .unwrap();
            notifier.await.unwrap();
        }
    }

    #[tokio::test]
    async fn cancellation_wakes_parked_wait() {
        let gate = Arc::new(Gate::new());
        let token = CancelToken::new();

        let waiter = {
            let gate = gate.clone();
            let token = token.clone();
            tokio::spawn(async move { gate.wait_until_cancellable(&token, || None::<()>).await })
        };

        tokio::task::yield_now().await;
        token.cancel();
        assert_eq!(waiter.await.unwrap(), Err(Cancelled));
    }

    #[tokio::test]
    async fn cancel_before_wait_resolves_immediately() {
        let gate = Gate::new();
        let token = CancelToken::new();
        token.cancel();
        assert_eq!(
            gate.wait_until_cancellable(&token, || None::<()>).await,
            Err(Cancelled)
        );
    }

    #[tokio::test]
    async fn predicate_wins_over_no_cancel() {
        let gate = Gate::new();
        let token = CancelToken::new();
        assert_eq!(gate.wait_until_cancellable(&token, || Some(1)).await, Ok(1));
    }

    #[tokio::test]
    async fn cancelled_future_resolves() {
        let token = CancelToken::new();
        let t2 = token.clone();
        let waiter = tokio::spawn(async move { t2.cancelled().await });
        tokio::task::yield_now().await;
        token.cancel();
        waiter.await.unwrap();
    }
}
