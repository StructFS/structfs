//! The fault table: typed replies for commands that never became a handle.
//!
//! A rejected command retains no state, so it must not occupy a slot in the
//! shared handle budget or an owner registration — otherwise a caller with no
//! valid request at all could lock every other view out for the whole
//! handle-age window. Faults live here instead, under two bounds:
//!
//! - **per principal**: each principal holds at most `limits.handles` faults;
//!   past that, *its own* oldest fault is dropped, so a noisy caller can only
//!   evict itself;
//! - **globally**: at most `limits.max_faults` across every principal; past
//!   that, the globally oldest fault is dropped, so many principals together
//!   cannot grow the table without bound.
//!
//! A fault stays readable (repeatably, like any handle) until its principal
//! releases it, it ages out after `limits.handle_age`, or one of the bounds
//! above drops it. A dropped or expired fault reads as `Fault::Closed`,
//! exactly like a released handle.
//!
//! Every operation is logarithmic in the table size or proportional to the
//! entries it removes: faults are indexed by insertion sequence and by
//! principal, so nothing scans the whole table under the state lock.

use std::collections::{BTreeMap, BTreeSet};

use tokio::time::Instant;

use crate::provider::Principal;
use crate::Fault;

struct Entry {
    principal: Principal,
    fault: Fault,
    seq: u64,
}

#[derive(Default)]
pub(crate) struct Faults {
    by_id: BTreeMap<String, Entry>,
    /// Insertion order. Every fault gets the same age, so this is also expiry
    /// order: the front is always both the oldest and the first to expire.
    by_seq: BTreeMap<u64, (String, Instant)>,
    by_principal: BTreeMap<Principal, BTreeSet<u64>>,
    next_seq: u64,
}

impl Faults {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    #[cfg(test)]
    pub(crate) fn count_for(&self, principal: &Principal) -> usize {
        self.by_principal.get(principal).map_or(0, BTreeSet::len)
    }

    /// The fault `id`, if `principal` owns it.
    pub(crate) fn get(&self, id: &str, principal: &Principal) -> Option<&Fault> {
        self.by_id
            .get(id)
            .filter(|e| &e.principal == principal)
            .map(|e| &e.fault)
    }

    /// Record a fault, first evicting to stay within both bounds.
    pub(crate) fn insert(
        &mut self,
        id: String,
        principal: &Principal,
        fault: Fault,
        expires: Instant,
        per_principal: usize,
        global: usize,
    ) {
        while self.by_principal.get(principal).map_or(0, BTreeSet::len) >= per_principal.max(1) {
            let oldest = self
                .by_principal
                .get(principal)
                .and_then(|seqs| seqs.first().copied());
            match oldest {
                Some(seq) => self.remove_seq(seq),
                None => break,
            }
        }
        while self.by_id.len() >= global.max(1) {
            match self.by_seq.first_key_value().map(|(seq, _)| *seq) {
                Some(seq) => self.remove_seq(seq),
                None => break,
            }
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.by_seq.insert(seq, (id.clone(), expires));
        self.by_principal
            .entry(principal.clone())
            .or_default()
            .insert(seq);
        self.by_id.insert(
            id,
            Entry {
                principal: principal.clone(),
                fault,
                seq,
            },
        );
    }

    /// Release `id` if `principal` owns it. Anything else — unknown, or
    /// someone else's — is a silent no-op, so probing reveals nothing.
    pub(crate) fn release(&mut self, id: &str, principal: &Principal) {
        if let Some(seq) = self
            .by_id
            .get(id)
            .filter(|e| &e.principal == principal)
            .map(|e| e.seq)
        {
            self.remove_seq(seq);
        }
    }

    /// Drop every fault that has aged out. Proportional to what it drops.
    pub(crate) fn expire(&mut self, now: Instant) {
        while let Some(seq) = self
            .by_seq
            .first_key_value()
            .filter(|(_, (_, expires))| *expires <= now)
            .map(|(seq, _)| *seq)
        {
            self.remove_seq(seq);
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self {
            next_seq: self.next_seq,
            ..Self::default()
        };
    }

    fn remove_seq(&mut self, seq: u64) {
        let Some((id, _)) = self.by_seq.remove(&seq) else {
            return;
        };
        if let Some(entry) = self.by_id.remove(&id) {
            if let Some(seqs) = self.by_principal.get_mut(&entry.principal) {
                seqs.remove(&seq);
                if seqs.is_empty() {
                    self.by_principal.remove(&entry.principal);
                }
            }
        }
    }
}
