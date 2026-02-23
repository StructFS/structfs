//! Namecode: Encode Unicode strings as valid programming language identifiers.
//!
//! Namecode encodes arbitrary Unicode strings into valid programming language
//! identifiers that work across Rust, Go, JavaScript, and Python. Think
//! "Punycode for variable names".
//!
//! # Key Properties
//!
//! - Encode/decode in O(n) time
//! - Idempotent: `encode(encode(x)) == encode(x)`
//! - Strict roundtrip: `encode(decode(s)) == s` for valid encodings
//!
//! # Examples
//!
//! ```
//! use namecode::{encode, decode};
//!
//! // Valid XID identifiers pass through unchanged
//! assert_eq!(encode("foo"), "foo");
//! assert_eq!(encode("café"), "café");
//! assert_eq!(encode("名前"), "名前");
//!
//! // Non-XID characters get encoded
//! let encoded = encode("hello world");
//! assert!(encoded.starts_with("_N_"));
//! assert_eq!(decode(&encoded).unwrap(), "hello world");
//!
//! // Roundtrip property
//! let original = "foo-bar";
//! let encoded = encode(original);
//! assert_eq!(decode(&encoded).unwrap(), original);
//! ```

#![warn(missing_docs)]

mod bootstring;
mod decode;
mod encode;

pub use decode::decode;
pub use encode::{encode, is_xid_identifier};

/// Errors that can occur during Namecode decoding.
///
/// # Examples
///
/// ```
/// use namecode::{decode, DecodeError};
///
/// match decode("not_encoded") {
///     Err(DecodeError::NotEncoded) => { /* expected */ }
///     other => panic!("unexpected: {:?}", other),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Input doesn't have the _N_ prefix.
    NotEncoded,
    /// Invalid character in encoded portion.
    InvalidDigit(char),
    /// Encoded data ended unexpectedly.
    UnexpectedEnd,
    /// Decoded to invalid Unicode codepoint.
    InvalidCodepoint(u32),
    /// Overflow during delta calculation.
    Overflow,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotEncoded => write!(f, "input is not a namecode-encoded string"),
            DecodeError::InvalidDigit(c) => write!(f, "invalid digit in encoded portion: '{}'", c),
            DecodeError::UnexpectedEnd => write!(f, "encoded data ended unexpectedly"),
            DecodeError::InvalidCodepoint(cp) => write!(f, "invalid Unicode codepoint: {}", cp),
            DecodeError::Overflow => write!(f, "overflow during decoding"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Basic Encoding Tests ====================

    #[test]
    fn test_encode_valid_xid_ascii() {
        assert_eq!(encode("foo"), "foo");
        assert_eq!(encode("bar123"), "bar123");
        assert_eq!(encode("_private"), "_private");
        assert_eq!(encode("CamelCase"), "CamelCase");
    }

    #[test]
    fn test_encode_valid_xid_unicode() {
        assert_eq!(encode("café"), "café");
        assert_eq!(encode("名前"), "名前");
        assert_eq!(encode("привет"), "привет");
    }

    #[test]
    fn test_encode_empty() {
        assert_eq!(encode(""), "");
    }

    #[test]
    fn test_encode_with_space() {
        let encoded = encode("hello world");
        assert!(encoded.starts_with("_N_"));
    }

    #[test]
    fn test_encode_with_hyphen() {
        let encoded = encode("foo-bar");
        assert!(encoded.starts_with("_N_"));
    }

    #[test]
    fn test_encode_with_multiple_non_basic() {
        let encoded = encode("a b-c");
        assert!(encoded.starts_with("_N_"));
    }

    #[test]
    fn test_encode_starts_with_digit() {
        let encoded = encode("123foo");
        assert!(encoded.starts_with("_N_"));
    }

    #[test]
    fn test_encode_just_underscore() {
        // Single underscore is a valid identifier, passes through
        assert_eq!(encode("_"), "_");
    }

    // ==================== Prefix/Delimiter Collision Tests ====================

    #[test]
    fn test_encode_prefix_collision() {
        let encoded = encode("_N_test");
        assert!(encoded.starts_with("_N_"));
        // Should not equal the input (would be ambiguous)
        assert_ne!(encoded, "_N_test");
    }

    #[test]
    fn test_encode_double_underscore_passthrough() {
        // foo__bar is a valid XID and doesn't start with _N_, so it passes through
        assert_eq!(encode("foo__bar"), "foo__bar");
    }

    // ==================== Roundtrip Tests ====================

    #[test]
    fn test_roundtrip_simple() {
        let cases = vec![
            "hello world",
            "foo-bar",
            "a b-c",
            "test@example",
            "with\ttab",
            "new\nline",
        ];

        for original in cases {
            let encoded = encode(original);
            let decoded = decode(&encoded)
                .unwrap_or_else(|e| panic!("decode failed for {}: {:?}", original, e));
            assert_eq!(
                decoded, original,
                "roundtrip failed for: {} (encoded: {})",
                original, encoded
            );
        }
    }

    #[test]
    fn test_roundtrip_unicode_non_xid() {
        let cases = vec!["hello→world", "price: $100", "50% off"];

        for original in cases {
            let encoded = encode(original);
            let decoded = decode(&encoded)
                .unwrap_or_else(|e| panic!("decode failed for {}: {:?}", original, e));
            assert_eq!(decoded, original);
        }
    }

    // ==================== Idempotency Tests ====================

    #[test]
    fn test_idempotent_valid_xid() {
        let s = "foo";
        assert_eq!(encode(&encode(s)), encode(s));
    }

    #[test]
    fn test_idempotent_encoded() {
        let s = "hello world";
        let once = encode(s);
        let twice = encode(&once);
        assert_eq!(once, twice);
    }

    // ==================== Identity Tests ====================

    #[test]
    fn test_identity_encode_decode() {
        // For valid encodings: encode(decode(s)) == s
        let original = "hello world";
        let encoded = encode(original);
        let decoded = decode(&encoded).unwrap();
        let re_encoded = encode(&decoded);
        assert_eq!(re_encoded, encoded);
    }

    // ==================== Decode Error Tests ====================

    #[test]
    fn test_decode_not_encoded() {
        assert_eq!(decode("foo"), Err(DecodeError::NotEncoded));
        assert_eq!(decode("hello world"), Err(DecodeError::NotEncoded));
    }

    #[test]
    fn test_decode_error_display() {
        // Test Display implementation for all error variants
        assert_eq!(
            DecodeError::NotEncoded.to_string(),
            "input is not a namecode-encoded string"
        );
        assert_eq!(
            DecodeError::InvalidDigit('X').to_string(),
            "invalid digit in encoded portion: 'X'"
        );
        assert_eq!(
            DecodeError::UnexpectedEnd.to_string(),
            "encoded data ended unexpectedly"
        );
        assert_eq!(
            DecodeError::InvalidCodepoint(0xFFFFFFFF).to_string(),
            "invalid Unicode codepoint: 4294967295"
        );
        assert_eq!(
            DecodeError::Overflow.to_string(),
            "overflow during decoding"
        );
    }

    #[test]
    fn test_decode_invalid_digit() {
        // The encoded portion contains invalid characters (6-9, symbols)
        // Note: uppercase letters are treated as lowercase in decode_digit
        let result = decode("_N_abc__6");
        assert!(
            matches!(result, Err(DecodeError::InvalidDigit(_))),
            "expected InvalidDigit, got {:?}",
            result
        );
    }

    #[test]
    fn test_decode_unexpected_end() {
        // Create a malformed encoding where the varint is incomplete
        // A digit >= threshold signals "more digits coming", but then we end
        // With initial bias of 72, threshold(32, 72) = 1, so any digit >= 1 needs more
        let result = decode("_N_abc__z"); // 'z' = 25, which is >= threshold
        assert!(matches!(result, Err(DecodeError::UnexpectedEnd)));
    }

    // ==================== is_xid_identifier Tests ====================

    #[test]
    fn test_is_xid_identifier() {
        assert!(is_xid_identifier("foo"));
        assert!(is_xid_identifier("_foo"));
        assert!(is_xid_identifier("foo123"));
        assert!(is_xid_identifier("café"));
        assert!(is_xid_identifier("名前"));
        assert!(is_xid_identifier("_")); // Single underscore is valid

        assert!(!is_xid_identifier(""));
        assert!(!is_xid_identifier("123"));
        assert!(!is_xid_identifier("foo bar"));
        assert!(!is_xid_identifier("foo-bar"));
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_all_non_basic() {
        let original = "   "; // All spaces
        let encoded = encode(original);
        assert!(encoded.starts_with("_N_"));
        let decoded = decode(&encoded).expect("decode failed");
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_single_char() {
        // Single basic char
        assert_eq!(encode("a"), "a");

        // Single non-basic char
        let encoded = encode(" ");
        assert!(encoded.starts_with("_N_"));
        assert_eq!(decode(&encoded).unwrap(), " ");
    }

    #[test]
    fn test_very_long_basic() {
        let long = "a".repeat(1000);
        assert_eq!(encode(&long), long);
    }

    // ==================== Additional Edge Cases ====================

    #[test]
    fn test_passthrough_double_underscore() {
        // foo__bar passes through unchanged (valid XID, no _N_ prefix)
        let original = "foo__bar";
        let encoded = encode(original);
        assert_eq!(encoded, original);
        // decode fails since it's not encoded
        assert!(decode(&encoded).is_err());
    }

    #[test]
    fn test_roundtrip_prefix_collision() {
        let original = "_N_test";
        let encoded = encode(original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_multiple_underscores_passthrough() {
        // All these are valid XID identifiers and don't start with _N_
        let cases = vec!["a__b", "a___b", "a____b", "__a", "___a", "____a"];

        for original in cases {
            let encoded = encode(original);
            assert_eq!(encoded, original, "should passthrough for: {}", original);
        }
    }

    #[test]
    fn test_just_underscores_passthrough() {
        // Multiple underscores ARE valid XID identifiers (underscore followed by XID_Continue,
        // and underscore IS XID_Continue). They pass through unchanged.
        let cases = vec!["__", "___", "____"];

        for original in cases {
            let encoded = encode(original);
            assert_eq!(encoded, original, "should passthrough: {}", original);
        }
    }

    #[test]
    fn test_single_underscore_passthrough() {
        // Single underscore is a valid identifier, passes through
        assert_eq!(encode("_"), "_");
    }
}

/// Test vectors from SPEC.md. If any of these break, the spec examples are stale.
#[cfg(test)]
mod spec_vectors {
    use super::*;

    // SPEC.md § Test Vectors > Passthrough Cases
    #[test]
    fn passthrough() {
        let cases: &[(&str, &str)] = &[
            ("foo", "foo"),
            ("_private", "_private"),
            ("café", "café"),
            ("名前", "名前"),
            ("CamelCase", "CamelCase"),
        ];
        for &(input, expected) in cases {
            assert_eq!(encode(input), expected, "passthrough: {:?}", input);
        }
    }

    // SPEC.md § Test Vectors > Encoding Cases
    #[test]
    fn encoding() {
        let cases: &[(&str, &str)] = &[
            ("hello world", "_N_helloworld__fa0b"),
            ("foo-bar", "_N_foobar__da1d"),
            ("a b c", "_N_abc__ba0bb0b"),
            ("123", "_N_123"),
            ("   ", "_N___a0ba0ba0b"),
        ];
        for &(input, expected) in cases {
            let encoded = encode(input);
            assert_eq!(encoded, expected, "encode: {:?}", input);
            // Verify roundtrip
            if encoded.starts_with("_N_") {
                assert_eq!(decode(&encoded).unwrap(), input, "roundtrip: {:?}", input);
            }
        }
    }

    // SPEC.md § Test Vectors > Edge Cases
    #[test]
    fn edge_cases() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            (" ", "_N___a0b"),
            ("a", "a"),
            ("_", "_"),
            ("_a", "_a"),
            ("__", "__"),
            ("___", "___"),
            ("foo__bar", "foo__bar"),
            ("_N_test", "_N__N_test"),
            ("__ _x", "_N__x__ba3la0ba3l"),
        ];
        for &(input, expected) in cases {
            let encoded = encode(input);
            assert_eq!(encoded, expected, "edge case: {:?}", input);
            // Verify roundtrip for encoded strings
            if encoded.starts_with("_N_") {
                assert_eq!(decode(&encoded).unwrap(), input, "roundtrip: {:?}", input);
            }
        }
    }

    // SPEC.md § Examples table
    #[test]
    fn decision_tree_examples() {
        assert_eq!(encode("foo"), "foo");
        assert_eq!(encode("cafe"), "cafe");
        assert_eq!(encode("café"), "café");
        assert_eq!(encode("名前"), "名前");
        assert_eq!(encode("foo__bar"), "foo__bar");
        assert_eq!(encode("hello world"), "_N_helloworld__fa0b");
        assert_eq!(encode("foo-bar"), "_N_foobar__da1d");
        assert_eq!(encode("123foo"), "_N_123foo");
        assert_eq!(encode("_N_test"), "_N__N_test");
        assert_eq!(encode("_"), "_");
        assert_eq!(encode(""), "");
    }

    // SPEC.md § Collision Handling examples
    #[test]
    fn collision_handling() {
        assert_eq!(encode("_N_test"), "_N__N_test");
        assert_eq!(decode("_N__N_test").unwrap(), "_N_test");

        assert_eq!(encode("foo__bar"), "foo__bar");
        assert_eq!(encode("__"), "__");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Roundtrip property: for strings that need encoding, decode(encode(s)) == s
        #[test]
        fn prop_roundtrip(s in ".*") {
            if !s.is_empty() {
                let encoded = encode(&s);
                // Only try to decode if it was actually encoded (has prefix)
                if encoded.starts_with("_N_") {
                    let decoded = decode(&encoded).unwrap_or_else(|e| {
                        panic!("decode failed for input '{}' with encoding '{}': {:?}", &s, &encoded, e)
                    });
                    prop_assert_eq!(&decoded, &s, "roundtrip failed for: {}", &s);
                } else {
                    // If not encoded, the output should equal input (passthrough)
                    prop_assert_eq!(&encoded, &s, "passthrough failed for: {}", &s);
                }
            }
        }

        /// Idempotency: encode(encode(x)) == encode(x)
        #[test]
        fn prop_idempotent(s in ".*") {
            let once = encode(&s);
            let twice = encode(&once);
            prop_assert_eq!(&once, &twice, "idempotency failed for: {}", &s);
        }

        /// Identity: for encoded strings, encode(decode(s)) == s
        #[test]
        fn prop_identity(s in ".*") {
            if !s.is_empty() {
                let encoded = encode(&s);
                // Only test identity for actually encoded strings
                if encoded.starts_with("_N_") {
                    let decoded = decode(&encoded).unwrap();
                    let re_encoded = encode(&decoded);
                    prop_assert_eq!(&re_encoded, &encoded, "identity failed for: {}", &s);
                }
            }
        }

        /// Valid output: encode produces valid XID identifiers (for non-empty)
        #[test]
        fn prop_valid_output(s in ".+") {
            let encoded = encode(&s);
            prop_assert!(
                is_xid_identifier(&encoded),
                "encode('{}') = '{}' is not a valid XID identifier",
                &s, &encoded
            );
        }

        /// XID passthrough: valid XID identifiers that don't start with _N_ pass through
        #[test]
        fn prop_xid_passthrough(s in "[a-zA-Z][a-zA-Z0-9_]*") {
            // Valid XID identifiers that don't start with _N_ pass through unchanged
            // (including those with __ in them)
            if !s.starts_with("_N_") && is_xid_identifier(&s) {
                let encoded = encode(&s);
                prop_assert_eq!(&encoded, &s, "XID passthrough failed for: {}", &s);
            }
        }

        /// Roundtrip with various character classes (strings that need encoding)
        #[test]
        fn prop_roundtrip_mixed(s in "[a-zA-Z0-9 \\-\\.,!@#$%^&*()]{1,50}") {
            // Ensure string contains at least one non-XID char
            if s.chars().any(|c| !unicode_ident::is_xid_continue(c)) {
                let encoded = encode(&s);
                prop_assert!(encoded.starts_with("_N_"), "expected encoding for: {}", &s);
                let decoded =
                    decode(&encoded).unwrap_or_else(|e| panic!("decode failed for {}: {:?}", &s, e));
                prop_assert_eq!(&decoded, &s);
            }
        }

        /// Roundtrip with Unicode (strings that need encoding due to non-XID chars)
        #[test]
        fn prop_roundtrip_unicode(s in "[a-z ]{1,10}") {
            // Use a simple pattern with spaces to ensure encoding happens
            if !s.is_empty() && s.contains(' ') {
                let encoded = encode(&s);
                prop_assert!(encoded.starts_with("_N_"));
                let decoded =
                    decode(&encoded).unwrap_or_else(|e| panic!("decode failed for {}: {:?}", &s, e));
                prop_assert_eq!(&decoded, &s);
            }
        }
    }
}

/// Kani verification harnesses for formal verification of namecode properties.
///
/// Run with: `cargo kani --package namecode`
#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use crate::bootstring::{
        adapt_bias, decode_digit, encode_digit, threshold, BASE, T_MAX, T_MIN,
    };

    // ==================== Bootstring Function Proofs ====================

    /// Verify encode_digit returns Some for valid inputs (0..32) and None otherwise
    #[kani::proof]
    fn verify_encode_digit_valid_range() {
        let digit: u32 = kani::any();

        let result = encode_digit(digit);

        if digit < 32 {
            assert!(
                result.is_some(),
                "encode_digit should return Some for digit < 32"
            );
            let c = result.unwrap();
            // Verify the character is in expected range
            assert!(
                ('a'..='z').contains(&c) || ('0'..='5').contains(&c),
                "encoded digit should be a-z or 0-5"
            );
        } else {
            assert!(
                result.is_none(),
                "encode_digit should return None for digit >= 32"
            );
        }
    }

    /// Verify decode_digit returns correct values for valid inputs
    #[kani::proof]
    fn verify_decode_digit_valid_range() {
        let c: char = kani::any();

        let result = decode_digit(c);

        match c {
            'a'..='z' => {
                assert!(result.is_some());
                assert!(result.unwrap() < 26);
            }
            'A'..='Z' => {
                // Case insensitive
                assert!(result.is_some());
                assert!(result.unwrap() < 26);
            }
            '0'..='5' => {
                assert!(result.is_some());
                let d = result.unwrap();
                assert!(d >= 26 && d < 32);
            }
            _ => {
                assert!(result.is_none());
            }
        }
    }

    /// Verify encode_digit and decode_digit are inverses
    #[kani::proof]
    fn verify_digit_roundtrip() {
        let digit: u32 = kani::any();
        kani::assume(digit < 32);

        let encoded = encode_digit(digit);
        assert!(encoded.is_some());

        let decoded = decode_digit(encoded.unwrap());
        assert!(decoded.is_some());
        assert_eq!(
            decoded.unwrap(),
            digit,
            "digit roundtrip should be identity"
        );
    }

    /// Verify threshold returns values in expected range
    #[kani::proof]
    fn verify_threshold_bounds() {
        let k: u32 = kani::any();
        let bias: u32 = kani::any();

        // Avoid overflow in threshold calculation
        kani::assume(k <= 10000);
        kani::assume(bias <= 10000);

        let t = threshold(k, bias);

        assert!(t >= T_MIN, "threshold should be >= T_MIN");
        assert!(t <= T_MAX, "threshold should be <= T_MAX");
    }

    /// Verify adapt_bias doesn't overflow and returns reasonable values
    #[kani::proof]
    fn verify_adapt_bias_no_overflow() {
        let delta: u32 = kani::any();
        let num_points: u32 = kani::any();
        let first_time: bool = kani::any();

        // Constrain to reasonable values to avoid very long verification
        kani::assume(delta <= 1_000_000);
        kani::assume(num_points > 0 && num_points <= 10000);

        let bias = adapt_bias(delta, num_points, first_time);

        // Bias should be a reasonable value (not overflowed)
        assert!(bias < 1_000_000, "bias should be bounded");
    }

    // ==================== XID Identifier Proofs ====================

    /// Verify is_xid_identifier returns false for empty string
    #[kani::proof]
    fn verify_is_xid_empty() {
        assert!(!is_xid_identifier(""));
    }

    /// Verify is_xid_identifier returns true for single underscore
    #[kani::proof]
    fn verify_is_xid_single_underscore() {
        assert!(is_xid_identifier("_"));
    }

    // ==================== Encode/Decode Proofs ====================

    /// Verify encode returns empty for empty input
    #[kani::proof]
    fn verify_encode_empty() {
        let result = encode("");
        assert!(result.is_empty());
    }

    /// Verify decode fails for non-encoded strings
    #[kani::proof]
    fn verify_decode_requires_prefix() {
        // Any string not starting with _N_ should fail
        let result = decode("abc");
        assert!(matches!(result, Err(DecodeError::NotEncoded)));
    }

    /// Verify idempotency for simple ASCII
    #[kani::proof]
    fn verify_idempotent_ascii() {
        // Test with a simple string that needs encoding
        let input = "a b";
        let once = encode(input);
        let twice = encode(&once);
        assert_eq!(once, twice, "encode should be idempotent");
    }

    /// Verify roundtrip for simple ASCII with space
    #[kani::proof]
    fn verify_roundtrip_simple() {
        let input = "hello world";
        let encoded = encode(input);
        let decoded = decode(&encoded);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), input);
    }

    /// Verify roundtrip for string with hyphen
    #[kani::proof]
    fn verify_roundtrip_hyphen() {
        let input = "foo-bar";
        let encoded = encode(input);
        let decoded = decode(&encoded);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), input);
    }

    /// Verify double underscore passes through (valid XID, no _N_ prefix)
    #[kani::proof]
    fn verify_double_underscore_passthrough() {
        let input = "a__b";
        let encoded = encode(input);
        // Should pass through unchanged (valid XID, no prefix collision)
        assert_eq!(encoded, input, "a__b should pass through");
    }

    /// Verify roundtrip for prefix collision
    #[kani::proof]
    fn verify_roundtrip_prefix_collision() {
        let input = "_N_x";
        let encoded = encode(input);
        // Should NOT equal the input
        assert_ne!(encoded, input);
        let decoded = decode(&encoded);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), input);
    }

    /// Verify valid XID identifiers pass through unchanged
    #[kani::proof]
    fn verify_xid_passthrough() {
        let input = "validIdentifier";
        let encoded = encode(input);
        assert_eq!(encoded, input, "valid XID should pass through");
    }

    /// Verify encode output is always valid XID (for non-empty input)
    #[kani::proof]
    fn verify_encode_produces_valid_xid() {
        let input = "test with spaces";
        let encoded = encode(input);
        assert!(
            is_xid_identifier(&encoded),
            "encode should produce valid XID"
        );
    }

    // ==================== Bounded Input Proofs ====================

    /// Verify encode doesn't panic for any single ASCII character
    #[kani::proof]
    fn verify_encode_single_ascii_no_panic() {
        let byte: u8 = kani::any();
        kani::assume(byte < 128); // ASCII only

        let s = String::from(byte as char);
        let _ = encode(&s); // Should not panic
    }

    /// Verify encode doesn't panic for two ASCII characters
    #[kani::proof]
    fn verify_encode_two_ascii_no_panic() {
        let b1: u8 = kani::any();
        let b2: u8 = kani::any();
        kani::assume(b1 < 128 && b2 < 128);

        let mut s = String::new();
        s.push(b1 as char);
        s.push(b2 as char);
        let _ = encode(&s); // Should not panic
    }

    /// Verify roundtrip for any single printable ASCII
    #[kani::proof]
    fn verify_roundtrip_single_printable() {
        let byte: u8 = kani::any();
        kani::assume(byte >= 32 && byte < 127); // Printable ASCII

        let input = String::from(byte as char);
        let encoded = encode(&input);

        if encoded.starts_with("_N_") {
            let decoded = decode(&encoded);
            assert!(decoded.is_ok());
            assert_eq!(decoded.unwrap(), input);
        } else {
            // Passed through unchanged (valid XID)
            assert_eq!(encoded, input);
        }
    }
}
