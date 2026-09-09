//! Deterministic sources
//! ([spec 12](https://github.com/StructFS/structfs/blob/main/isotope/spec/12-determinism.md)).
//!
//! Determinism and transcription are orthogonal features. This module is
//! the determinism half: virtual providers for the `input` mounts the
//! `/iso` surface serves — seeded entropy and a virtual clock — so that
//! two live runs with the same seed are the same run, with no transcript
//! involved. Mix and match with [`crate::TranscriptMode`]: a seeded run
//! can be recorded (two recordings with one seed are byte-identical,
//! which is how the tests *prove* the determinism), and a nondeterministic
//! run can be recorded too — the transcript is then a record, not a
//! promise.
//!
//! The derivations are pinned by spec 12's *Standard Virtual
//! Providers* and held cross-runtime by the committed seeded fixture:
//! per-block streams derive from the seed and the block's stable
//! assembly-scoped key, `time/after` waits in virtual time, and block
//! ids derive from keys — so `iso/self/id`, like every other input,
//! answers the same on every run.
//!
//! What this does not virtualize: scheduling. Blocks run on OS threads
//! and the cross-block interleaving of mailbox events is the host's; a
//! single block that consumes only its own boundary is deterministic
//! under a seed, an assembly's cross-block interleaving is not yet.
//! Epoch-based metering can also kill a slow guest on wall-clock
//! grounds — it never changes a surviving run's values, but whether a
//! run survives a timeout is the host's, not the seed's.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// The runtime's determinism mode: how the `/iso` surface sources what
/// only the world can normally answer.
#[derive(Clone, Default)]
pub enum Determinism {
    /// Real clock, real entropy.
    #[default]
    Live,
    /// Seeded entropy and a virtual clock. Each block derives its own
    /// stream from the seed and its name, so streams are stable per
    /// block and distinct across blocks.
    Seeded { seed: u64 },
}

impl Determinism {
    /// The sources for one block.
    pub(crate) fn sources_for(&self, block: &str) -> IsoSources {
        match self {
            Determinism::Live => IsoSources {
                entropy: None,
                clock: None,
            },
            Determinism::Seeded { seed } => IsoSources {
                entropy: Some(Mutex::new(seed ^ fnv1a(block.as_bytes()))),
                clock: Some(VirtualClock::default()),
            },
        }
    }
}

/// Per-block sources held by the iso surface. `None` means the world.
pub(crate) struct IsoSources {
    /// splitmix64 state for `/iso/random`.
    entropy: Option<Mutex<u64>>,
    clock: Option<VirtualClock>,
}

impl IsoSources {
    /// The next `n` deterministic bytes, or `None` when entropy is live.
    pub(crate) fn entropy_bytes(&self, n: usize) -> Option<Vec<u8>> {
        let state = self.entropy.as_ref()?;
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        let mut bytes = Vec::with_capacity(n);
        while bytes.len() < n {
            bytes.extend_from_slice(&splitmix64(&mut state).to_le_bytes());
        }
        bytes.truncate(n);
        Some(bytes)
    }

    /// The virtual now in unix nanoseconds, or `None` when time is live.
    /// Every read advances the clock one tick, so time observably moves.
    pub(crate) fn now_unix_ns(&self) -> Option<i64> {
        let clock = self.clock.as_ref()?;
        Some(clock.epoch_ns + clock.tick_ns * clock.ticks.fetch_add(1, Ordering::SeqCst))
    }

    /// Virtual nanoseconds since block start, or `None` when live.
    pub(crate) fn monotonic_ns(&self) -> Option<i64> {
        self.now_unix_ns()
            .map(|ns| ns - self.clock.as_ref().expect("checked").epoch_ns)
    }

    /// Under the virtual clock, `time/after/{ms}` completes immediately
    /// after advancing virtual time by the requested span — simulation
    /// semantics: a seeded run waits in virtual time, not wall time, so
    /// timers are a function of the run rather than of the host's
    /// scheduler. Answers false when time is live and the caller should
    /// really sleep.
    pub(crate) fn advance_after(&self, ms: u64) -> bool {
        let Some(clock) = &self.clock else {
            return false;
        };
        let ticks = (ms as i64 * 1_000_000) / clock.tick_ns;
        clock.ticks.fetch_add(ticks, Ordering::SeqCst);
        true
    }

    /// Fast-forward past a replayed prefix (spec 12 seek): advance the
    /// entropy stream by `words` draws and the virtual clock by `ticks`
    /// reads, so a seeded run that hands off continues exactly where a
    /// straight run would be. A no-op for live sources.
    pub(crate) fn fast_forward(&self, words: u64, ticks: u64) {
        if let Some(state) = &self.entropy {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            for _ in 0..words {
                splitmix64(&mut state);
            }
        }
        if let Some(clock) = &self.clock {
            clock.ticks.fetch_add(ticks as i64, Ordering::SeqCst);
        }
    }
}

/// A clock whose only obligation is to be a function of how often it is
/// read: it starts at a fixed epoch and advances one tick per read.
struct VirtualClock {
    epoch_ns: i64,
    tick_ns: i64,
    ticks: AtomicI64,
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self {
            // 2000-01-01T00:00:00Z: recognizably not the real clock,
            // valid to every date library.
            epoch_ns: 946_684_800_000_000_000,
            tick_ns: 1_000_000, // 1ms per read
            ticks: AtomicI64::new(0),
        }
    }
}

/// splitmix64: tiny, well-distributed, and needs no dependency — the
/// point is reproducibility, not cryptography, and the spec's entropy
/// path makes no stronger promise than "the host decides".
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_sources_are_stable_per_block_and_distinct_across_blocks() {
        let mode = Determinism::Seeded { seed: 42 };
        let a1 = mode.sources_for("api").entropy_bytes(16).unwrap();
        let a2 = mode.sources_for("api").entropy_bytes(16).unwrap();
        let b = mode.sources_for("cache").entropy_bytes(16).unwrap();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
    }

    #[test]
    fn the_virtual_clock_moves_one_tick_per_read() {
        let sources = Determinism::Seeded { seed: 7 }.sources_for("api");
        let first = sources.now_unix_ns().unwrap();
        let second = sources.now_unix_ns().unwrap();
        assert_eq!(second - first, 1_000_000);
        assert_eq!(first, 946_684_800_000_000_000);
    }

    #[test]
    fn live_mode_defers_to_the_world() {
        let sources = Determinism::Live.sources_for("api");
        assert!(sources.entropy_bytes(8).is_none());
        assert!(sources.now_unix_ns().is_none());
        assert!(sources.monotonic_ns().is_none());
    }
}
