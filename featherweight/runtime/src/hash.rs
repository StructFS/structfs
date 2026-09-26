//! The two tiny, dependency-free hashes the runtime pins by specification
//! (spec 12): splitmix64 for seeded streams (entropy and the simulation
//! schedule) and fnv1a-64 for stream derivation and write digests. Their
//! outputs are part of committed cross-host fixtures, so there is exactly
//! one copy of each.

/// splitmix64: tiny and well-distributed. The point is reproducibility,
/// not cryptography.
pub(crate) fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// fnv1a-64 over `bytes`.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
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
    fn pinned_vectors() {
        // Values the committed seeded fixture depends on.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        let mut state = 0;
        assert_eq!(splitmix64(&mut state), 0xe220_a839_7b1d_cdaf);
    }
}
