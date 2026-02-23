//! Bootstring algorithm constants and helpers.
//!
//! This is an adaptation of the Bootstring algorithm (RFC 3492) for encoding
//! Unicode strings as valid identifiers. We use base 32 with the alphabet
//! a-z (26) + 0-5 (6) = 32 characters, all valid in identifiers.

/// Base for variable-length integer encoding.
pub(crate) const BASE: u32 = 32;

/// Minimum threshold value.
pub(crate) const T_MIN: u32 = 1;

/// Maximum threshold value.
pub(crate) const T_MAX: u32 = 26;

/// Skew factor for bias adaptation.
pub(crate) const SKEW: u32 = 38;

/// Damping factor for first adaptation.
pub(crate) const DAMP: u32 = 700;

/// Initial bias value.
pub(crate) const INITIAL_BIAS: u32 = 72;

/// The encoding alphabet: a-z (0-25) + 0-5 (26-31).
const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz012345";

/// Adapt bias after encoding/decoding a delta.
///
/// This function implements the bias adaptation algorithm from RFC 3492.
/// It adjusts the bias to improve encoding efficiency based on:
/// - delta: the delta value just processed
/// - num_points: number of code points handled so far
/// - first_time: whether this is the first adaptation
pub(crate) fn adapt_bias(mut delta: u32, num_points: u32, first_time: bool) -> u32 {
    // Scale delta down
    delta = if first_time { delta / DAMP } else { delta / 2 };

    // Compensate for the length of the string
    delta += delta / num_points;

    // Find the number of divisions needed
    let mut k = 0u32;
    let base_minus_tmin = BASE - T_MIN;
    let threshold = (base_minus_tmin * T_MAX) / 2;

    while delta > threshold {
        delta /= base_minus_tmin;
        k += BASE;
    }

    k + ((base_minus_tmin + 1) * delta) / (delta + SKEW)
}

/// Encode a digit value (0-31) to its character representation.
///
/// Returns `None` if the digit is out of range.
pub(crate) fn encode_digit(d: u32) -> Option<char> {
    if d < 32 {
        Some(ALPHABET[d as usize] as char)
    } else {
        None
    }
}

/// Decode a character to its digit value (0-31).
///
/// Returns `None` if the character is not in the alphabet.
pub(crate) fn decode_digit(c: char) -> Option<u32> {
    match c {
        'a'..='z' => Some(c as u32 - 'a' as u32),
        'A'..='Z' => Some(c as u32 - 'A' as u32), // Case insensitive
        '0'..='5' => Some(c as u32 - '0' as u32 + 26),
        _ => None,
    }
}

/// Calculate the threshold for a given position k and bias.
pub(crate) fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias + T_MIN {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_digit() {
        // a-z maps to 0-25
        assert_eq!(encode_digit(0), Some('a'));
        assert_eq!(encode_digit(25), Some('z'));

        // 0-5 maps to 26-31
        assert_eq!(encode_digit(26), Some('0'));
        assert_eq!(encode_digit(31), Some('5'));

        // Out of range
        assert_eq!(encode_digit(32), None);
    }

    #[test]
    fn test_decode_digit() {
        // a-z maps to 0-25
        assert_eq!(decode_digit('a'), Some(0));
        assert_eq!(decode_digit('z'), Some(25));

        // Case insensitive
        assert_eq!(decode_digit('A'), Some(0));
        assert_eq!(decode_digit('Z'), Some(25));

        // 0-5 maps to 26-31
        assert_eq!(decode_digit('0'), Some(26));
        assert_eq!(decode_digit('5'), Some(31));

        // Invalid
        assert_eq!(decode_digit('6'), None);
        assert_eq!(decode_digit('-'), None);
    }

    #[test]
    fn test_roundtrip() {
        for d in 0..32 {
            let c = encode_digit(d).unwrap();
            assert_eq!(decode_digit(c), Some(d));
        }
    }

    #[test]
    fn test_threshold() {
        // k <= bias + T_MIN => T_MIN
        assert_eq!(threshold(1, 72), T_MIN);
        assert_eq!(threshold(73, 72), T_MIN);

        // k >= bias + T_MAX => T_MAX
        assert_eq!(threshold(100, 72), T_MAX);

        // Otherwise k - bias
        assert_eq!(threshold(80, 72), 8);
    }

    #[test]
    fn test_adapt_bias() {
        // Test with some known values
        let bias = adapt_bias(0, 1, true);
        assert!(bias < BASE);

        // First time should divide by DAMP
        let bias1 = adapt_bias(1000, 1, true);
        let bias2 = adapt_bias(1000, 1, false);
        // First time adaptation produces different result
        assert_ne!(bias1, bias2);
    }
}
