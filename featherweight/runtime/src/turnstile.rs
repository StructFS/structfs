//! The deterministic scheduler: Antithesis-style simulation
//! ([spec 12](https://github.com/StructFS/structfs/blob/main/isotope/spec/12-determinism.md)).
//!
//! Under [`crate::Determinism::Simulation`], cross-block interleaving is
//! a function of the seed, not of the OS scheduler. One turn token
//! exists; a block executes only while holding it, and releases it only
//! when it parks (an empty mailbox, an in-flight call) or exits. Every
//! wake — a request landing in a mailbox, a response resolving a call —
//! happens during the *waker's* turn, so the runnable set at every
//! scheduling decision is a function of the run so far; the next
//! runnable block is drawn from the seeded generator. Same seed, same
//! interleaving; a different seed *explores* a different one, which is
//! what makes race-hunting a matter of iterating seeds.
//!
//! Deadlock becomes detectable instead of silent — but a deadlock is a
//! *dependency cycle*, not idleness. A block parked on its own mailbox
//! is waiting for work that may legitimately never come (a server whose
//! clients have all exited, or one awaiting external input); the
//! schedule simply goes quiet, and a host poke or shutdown revives it.
//! A block parked on a *call to a peer* has a dependency, and when no
//! block is runnable while some block is call-parked, no wake can ever
//! come (all wakes happen on turns) — that is the cycle, and the
//! runtime shuts the assembly down loudly.
//!
//! What simulation does not cover, by construction: host-driven
//! operations (an embedder calling into the assembly is external
//! input), blocking host stdio, `iso/timers` (refused under
//! simulation), and `proc/outstanding/{id}/wait`. A simulated assembly
//! should be self-driving.

use std::collections::{BTreeMap, BTreeSet};

/// Why a block released its turn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkKind {
    /// Idle on its own mailbox — no dependency; quiescence, not deadlock.
    Mailbox,
    /// Awaiting a peer's response to a call — a dependency.
    Call,
}
use std::sync::{Arc, Mutex};

// The block key the current OS thread is executing, when it is a
// block thread. Host threads carry `None` and are never scheduled.
std::thread_local! {
    static CURRENT_BLOCK: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_current_block(key: Option<String>) {
    CURRENT_BLOCK.with(|current| *current.borrow_mut() = key);
}

pub(crate) fn current_block() -> Option<String> {
    ASYNC_BLOCK
        .try_with(Clone::clone)
        .ok()
        .flatten()
        .or_else(|| CURRENT_BLOCK.with(|current| current.borrow().clone()))
}

tokio::task_local! {
    static ASYNC_BLOCK: Option<String>;
}

/// Identity follows a future across worker threads, never ambient thread state.
pub(crate) async fn scope_block<F: std::future::Future>(
    key: Option<String>,
    future: F,
) -> F::Output {
    ASYNC_BLOCK.scope(key, future).await
}

/// Explicit identity transfer at the synchronous adapter boundary.
pub(crate) async fn blocking<F, T>(work: F) -> Result<T, tokio::task::JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let key = current_block();
    tokio::task::spawn_blocking(move || {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                set_current_block(None);
            }
        }
        set_current_block(key);
        let _clear = Clear;
        work()
    })
    .await
}

struct TurnState {
    /// splitmix64 state: the schedule is drawn from the seed.
    rng: u64,
    holder: Option<String>,
    /// Ready to run, awaiting a grant.
    runnable: BTreeSet<String>,
    /// Parked, with whether the park is a dependency on a peer (a
    /// call) rather than idleness (a mailbox wait). Only dependency
    /// parks arm deadlock detection.
    parked: BTreeMap<String, ParkKind>,
    /// Exited for good: wakes for these are ignored — granting a turn
    /// to a thread that no longer exists would wedge the schedule.
    gone: BTreeSet<String>,
    grants: BTreeMap<String, Arc<tokio::sync::Notify>>,
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The turn token and the seeded schedule.
pub(crate) struct Turnstile {
    state: Mutex<TurnState>,
    on_deadlock: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Turnstile {
    pub(crate) fn new(seed: u64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TurnState {
                // A stream of its own, apart from any block's entropy.
                rng: seed ^ 0x7375_6c61_7469_6f6e, // "sulation"
                holder: None,
                runnable: BTreeSet::new(),
                parked: BTreeMap::new(),
                gone: BTreeSet::new(),
                grants: BTreeMap::new(),
            }),
            on_deadlock: Mutex::new(None),
        })
    }

    /// What to do when the schedule wedges: no holder, nothing
    /// runnable, blocks parked. Set once by the runtime.
    pub(crate) fn set_deadlock_handler(&self, handler: impl Fn() + Send + Sync + 'static) {
        *self.on_deadlock.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(handler));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TurnState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Pick and grant the next turn if none is held. Returns true when
    /// the schedule is wedged (deadlock).
    fn schedule(state: &mut TurnState) -> bool {
        if state.holder.is_some() {
            return false;
        }
        if state.runnable.is_empty() {
            // Quiescent (only mailbox idleness) is not a wedge: a host
            // poke or shutdown will revive it. A pending call with
            // nothing runnable is a dependency cycle.
            return state.parked.values().any(|kind| *kind == ParkKind::Call);
        }
        let pick = (splitmix64(&mut state.rng) as usize) % state.runnable.len();
        let key = state
            .runnable
            .iter()
            .nth(pick)
            .expect("picked within len")
            .clone();
        state.runnable.remove(&key);
        state.holder = Some(key.clone());
        if let Some(grant) = state.grants.get(&key) {
            grant.notify_one();
        }
        false
    }

    fn after(&self, deadlocked: bool) {
        if deadlocked {
            tracing::error!(
                "deterministic deadlock: no block runnable, none running, some parked — \
                 shutting the assembly down"
            );
            if let Some(handler) = &*self.on_deadlock.lock().unwrap_or_else(|e| e.into_inner()) {
                handler();
            }
        }
    }

    /// Enroll a block in the schedule as runnable, without deciding
    /// anything yet. Called synchronously where blocks are created —
    /// instantiation order, which is deterministic — so the first
    /// scheduling decision never races thread startup.
    pub(crate) fn enroll(&self, key: &str) {
        let mut state = self.lock();
        state
            .grants
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()));
        state.runnable.insert(key.to_string());
    }

    /// Make the first scheduling decision once every initial block is
    /// enrolled. A no-op while a turn is held (a spawner instantiating
    /// children mid-run), so it is always safe to call.
    pub(crate) fn launch(&self) {
        let mut state = self.lock();
        let wedged = Self::schedule(&mut state);
        drop(state);
        self.after(wedged);
    }

    /// A block thread entering the schedule: wait for the turn its
    /// enrollment earned.
    pub(crate) async fn start(&self, key: &str) {
        self.wait_turn(key).await;
    }

    /// A seeded interleaving point: give the schedule a chance to run
    /// someone else (or pick this block again). Every boundary
    /// operation yields, which is what makes racy interleavings a
    /// function of the seed rather than of who never happened to park.
    pub(crate) async fn yield_now(&self, key: &str) {
        {
            let mut state = self.lock();
            if state.holder.as_deref() == Some(key) {
                state.holder = None;
            }
            state.runnable.insert(key.to_string());
            let wedged = Self::schedule(&mut state);
            drop(state);
            self.after(wedged);
        }
        self.wait_turn(key).await;
    }

    /// Park: the caller is waiting for an event only a turn-holder can
    /// deliver. Releases the turn and schedules the next block.
    /// `kind` says whether this park is a dependency (a call) or mere
    /// idleness (a mailbox wait) — only the former arms deadlock.
    pub(crate) fn park(&self, key: &str, kind: ParkKind) {
        let mut state = self.lock();
        if state.holder.as_deref() == Some(key) {
            state.holder = None;
        }
        state.runnable.remove(key);
        state.parked.insert(key.to_string(), kind);
        let wedged = Self::schedule(&mut state);
        drop(state);
        self.after(wedged);
    }

    /// A response to `key`'s pending call arrived: the dependency is
    /// satisfied, so it becomes runnable whatever it was parked on.
    pub(crate) fn wake_response(&self, key: &str) {
        self.wake(key, true);
    }

    /// A mailbox event for `key` arrived (a request, a signal, a
    /// shutdown). It wakes an *idle* block — one parked on its mailbox,
    /// or not parked — but never a block parked on a call: that block
    /// is mid-call and will drain its mailbox when the call returns.
    /// Waking it would grant a turn it cannot use, and, worse, hide the
    /// dependency cycle this is exactly meant to leave wedged.
    pub(crate) fn wake_mailbox(&self, key: &str) {
        self.wake(key, false);
    }

    fn wake(&self, key: &str, response: bool) {
        let mut state = self.lock();
        if state.holder.as_deref() == Some(key) || state.gone.contains(key) {
            return;
        }
        if !response && matches!(state.parked.get(key), Some(ParkKind::Call)) {
            return;
        }
        state.parked.remove(key);
        state.runnable.insert(key.to_string());
        let wedged = Self::schedule(&mut state);
        drop(state);
        self.after(wedged);
    }

    /// Wait until this block holds the turn (having been made
    /// runnable). The notify is re-checked in a loop: grants store a
    /// permit, so a grant issued before the wait began is not lost.
    pub(crate) async fn wait_turn(&self, key: &str) {
        loop {
            let grant = {
                let state = self.lock();
                if state.holder.as_deref() == Some(key) {
                    return;
                }
                state.grants.get(key).cloned()
            };
            match grant {
                Some(grant) => grant.notified().await,
                None => return, // exited under us (shutdown)
            }
        }
    }

    /// Leave the schedule for good. Later wakes for this key are
    /// ignored: nothing is left to run.
    pub(crate) fn exit(&self, key: &str) {
        let mut state = self.lock();
        if state.holder.as_deref() == Some(key) {
            state.holder = None;
        }
        state.runnable.remove(key);
        state.parked.remove(key);
        state.grants.remove(key);
        state.gone.insert(key.to_string());
        let wedged = Self::schedule(&mut state);
        drop(state);
        self.after(wedged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain a schedule by repeatedly parking the holder: the order
    /// turns are granted is a function of the seed alone.
    fn granted_order(seed: u64, keys: &[&str]) -> Vec<String> {
        let turnstile = Turnstile::new(seed);
        {
            let mut state = turnstile.lock();
            for key in keys {
                state
                    .grants
                    .insert((*key).to_string(), Arc::new(tokio::sync::Notify::new()));
                state.runnable.insert((*key).to_string());
            }
            let wedged = Turnstile::schedule(&mut state);
            assert!(!wedged);
        }
        let mut order = Vec::new();
        loop {
            // Bind outside the loop condition: a while-let scrutinee
            // temporary would hold the mutex guard across `exit`.
            let holder = turnstile.lock().holder.clone();
            let Some(holder) = holder else { break };
            order.push(holder.clone());
            turnstile.exit(&holder);
        }
        order
    }

    #[test]
    fn the_schedule_is_a_function_of_the_seed() {
        let keys = ["a/w1", "a/w2", "a/w3", "a/w4"];
        assert_eq!(granted_order(42, &keys), granted_order(42, &keys));
        // Different seeds explore different interleavings (these two
        // verified distinct; the space is 4! = 24 orders).
        assert_ne!(granted_order(42, &keys), granted_order(1, &keys));
    }

    #[test]
    fn a_wedged_schedule_fires_the_deadlock_handler() {
        let turnstile = Turnstile::new(7);
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = fired.clone();
        turnstile.set_deadlock_handler(move || {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        {
            let mut state = turnstile.lock();
            state
                .grants
                .insert("a/hermit".to_string(), Arc::new(tokio::sync::Notify::new()));
            state.runnable.insert("a/hermit".to_string());
            Turnstile::schedule(&mut state);
        }
        // A lone call-park with nothing to answer it is a wedge; a
        // mailbox park would be mere idleness and not fire.
        turnstile.park("a/hermit", ParkKind::Call);
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
        // A response wake recovers the schedule.
        turnstile.wake_response("a/hermit");
        assert_eq!(turnstile.lock().holder.as_deref(), Some("a/hermit"));
    }

    #[test]
    fn a_lone_mailbox_park_is_idle_not_a_deadlock() {
        let turnstile = Turnstile::new(7);
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = fired.clone();
        turnstile.set_deadlock_handler(move || {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        {
            let mut state = turnstile.lock();
            state
                .grants
                .insert("a/server".to_string(), Arc::new(tokio::sync::Notify::new()));
            state.runnable.insert("a/server".to_string());
            Turnstile::schedule(&mut state);
        }
        // An idle server parked on its mailbox: quiescence, not a wedge.
        turnstile.park("a/server", ParkKind::Mailbox);
        assert!(!fired.load(std::sync::atomic::Ordering::SeqCst));
        // A host poke (a mailbox wake) revives it.
        turnstile.wake_mailbox("a/server");
        assert_eq!(turnstile.lock().holder.as_deref(), Some("a/server"));
    }
}
