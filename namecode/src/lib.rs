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

mod bootstring;
mod decode;
mod encode;

pub use decode::decode;
pub use encode::{encode, is_xid_identifier};

/// Errors that can occur during Namecode decoding.
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

/// Quick check if a string appears to be Namecode-encoded.
///
/// This checks if the string starts with the `_N_` prefix.
/// Note: This doesn't validate that the encoding is well-formed.
pub fn is_encoded(input: &str) -> bool {
    input.starts_with(encode::PREFIX)
}

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
        assert!(is_encoded(&encoded));
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
        // Just "_" is not a valid identifier
        let encoded = encode("_");
        assert!(encoded.starts_with("_N_"));
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
    fn test_encode_delimiter_collision() {
        let encoded = encode("foo__bar");
        assert!(encoded.starts_with("_N_"));
        // Should not equal the input (would be ambiguous)
        assert_ne!(encoded, "foo__bar");
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

        assert!(!is_xid_identifier(""));
        assert!(!is_xid_identifier("123"));
        assert!(!is_xid_identifier("foo bar"));
        assert!(!is_xid_identifier("foo-bar"));
        assert!(!is_xid_identifier("_"));
    }

    // ==================== is_encoded Tests ====================

    #[test]
    fn test_is_encoded() {
        assert!(is_encoded("_N_foo"));
        assert!(is_encoded("_N_test__abc"));

        assert!(!is_encoded("foo"));
        assert!(!is_encoded("_foo"));
        assert!(!is_encoded("N_foo"));
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
    fn test_roundtrip_delimiter_collision() {
        let original = "foo__bar";
        let encoded = encode(original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_roundtrip_prefix_collision() {
        let original = "_N_test";
        let encoded = encode(original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_roundtrip_multiple_underscores() {
        let cases = vec!["a__b", "a___b", "a____b", "__", "___", "____"];

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

        /// XID passthrough: valid XID identifiers that don't conflict pass through
        #[test]
        fn prop_xid_passthrough(s in "[a-zA-Z][a-zA-Z0-9_]*") {
            // Only test if it doesn't conflict with our encoding
            // Note: exclude leading underscore as "_" alone is not valid
            if !s.starts_with("_N_") && !s.contains("__") && is_xid_identifier(&s) {
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
