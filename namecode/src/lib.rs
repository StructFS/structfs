//! Namecode: Encode Unicode strings as valid programming language identifiers.
//!
//! Namecode encodes arbitrary Unicode strings into valid programming language
//! identifiers that work across Rust, Go, JavaScript, and Python. Think
//! "Punycode for variable names".
//!
//! # Key Properties
//!
//! - Encode/decode in O(n) time
//! - **Lossless**: `decode(encode(s)) == s` whenever `encode(s) != s`; a
//!   passthrough (`encode(s) == s`) stands for itself, and `decode` answers
//!   `NotEncoded` for it. So "decode, falling back to the input on
//!   `NotEncoded`" inverts `encode` for *every* `s`, the empty string and
//!   look-alike encodings included
//! - **Forced form is a bijection**: `decode(encode_forced(s)) == s` for every
//!   `s`, and `encode_forced(decode(t)) == t` for every `t` that `decode`
//!   accepts — each value has exactly one encoding, and non-canonical
//!   spellings (uppercase digits, redundant delimiters) are rejected
//! - **Stable passthrough**: `encode(t) == t` exactly when `t` is a valid
//!   UAX 31 identifier that does not start with `_N_`
//!
//! Note that `encode(decode(t))` need not equal `t`: `decode("_N_foo")` is
//! `"foo"`, which `encode` passes through. `_N_foo` is the canonical *forced*
//! encoding of `foo`, not something `encode` ever produces.
//!
//! `encode` is deliberately *not* idempotent: `encode(encode(x))` encodes
//! twice, because passing an encoding through unchanged would make it
//! impossible to tell an encoded string from a literal one that happens to
//! look encoded. Use [`is_encoded`] when you need that distinction.
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
pub use encode::{encode, encode_forced, is_encoded, is_xid_identifier};

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
#[non_exhaustive]
pub enum DecodeError {
    /// Input doesn't have the _N_ prefix, or has it but is structurally not
    /// an encoding (a basic portion containing characters the encoder never
    /// emits, for instance).
    NotEncoded,
    /// Invalid character in encoded portion. Note that the bootstring
    /// alphabet is lowercase-only: `'A'` is as invalid as `'!'`.
    InvalidDigit(char),
    /// Encoded data ended unexpectedly.
    UnexpectedEnd,
    /// Decoded to invalid Unicode codepoint.
    InvalidCodepoint(u32),
    /// Overflow during delta calculation.
    Overflow,
    /// The input parses, but it is not the canonical encoding of the value it
    /// parses to — re-encoding that value yields a different string. Accepting
    /// it would give one value two encodings.
    NonCanonical,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotEncoded => write!(f, "input is not a namecode-encoded string"),
            DecodeError::InvalidDigit(c) => write!(f, "invalid digit in encoded portion: '{}'", c),
            DecodeError::UnexpectedEnd => write!(f, "encoded data ended unexpectedly"),
            DecodeError::InvalidCodepoint(cp) => write!(f, "invalid Unicode codepoint: {}", cp),
            DecodeError::Overflow => write!(f, "overflow during decoding"),
            DecodeError::NonCanonical => {
                write!(f, "input is not the canonical encoding of its value")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A corpus of strings that between them exercise every branch of the
    /// encoder: passthrough, prefix collision, delimiter collision, leading
    /// digits, whitespace, punctuation, astral planes, combining marks, ZWJ
    /// sequences and lengths past every internal buffer heuristic.
    fn corpus() -> Vec<String> {
        let mut cases: Vec<String> = [
            // Empty and single characters
            "",
            "a",
            "A",
            "_",
            " ",
            "-",
            "0",
            // Underscore runs (all valid identifiers)
            "__",
            "___",
            "____",
            "a__b",
            "__ _x",
            // Prefix collisions
            "_N",
            "_N_",
            "_N_x",
            "_N_test",
            "_N__N_test",
            "_N_hello world",
            // Strings that look like encodings but are not canonical ones
            "_N_helloworld__fa0b",
            "_N_helloworld__FA0B",
            "_N_abc__9",
            "_N___",
            // Numbers and leading digits
            "123",
            "007",
            "1abc",
            "abc1",
            "3.14159",
            // Whitespace and punctuation
            "hello world",
            "   ",
            " leading",
            "trailing ",
            "foo-bar",
            "foo.bar",
            "foo/bar",
            "a/b/c",
            "foo@bar.com",
            "50% off",
            "price: $100",
            "with\ttab",
            "new\nline",
            "null\u{0}byte",
            // Mixed case
            "CamelCase",
            "mixedCASE123",
            "SCREAMING_SNAKE",
            // Non-ASCII
            "café",
            "CAFÉ",
            "名前",
            "привет",
            "مرحبا",
            "hello→world",
            "ＦＵＬＬＷＩＤＴＨ",
            // Combining marks, emoji, ZWJ sequences, flags
            "e\u{301}",
            "🦀",
            "oxide-🦀",
            "👨\u{200d}👩\u{200d}👧\u{200d}👦",
            "🇺🇸",
            "\u{200b}",
            "\u{feff}",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // Very long inputs, basic and mixed
        cases.push("a".repeat(1000));
        cases.push("_N_".to_string() + &"a".repeat(500));
        cases.push("a b".repeat(300));
        cases.push("🦀".repeat(200));
        cases
    }

    /// The contract every caller relies on, checked over the whole corpus:
    /// the encoding is always a valid identifier, and decoding it (falling
    /// back to the literal for passthrough forms, exactly as
    /// `PathComponent::decode` does) returns the original string.
    #[test]
    fn corpus_roundtrip() {
        for original in corpus() {
            let encoded = encode(&original);

            assert!(
                is_xid_identifier(&encoded),
                "encode({original:?}) = {encoded:?} is not a valid identifier"
            );

            let recovered = match decode(&encoded) {
                Ok(value) => value,
                Err(DecodeError::NotEncoded) => encoded.clone(),
                Err(e) => panic!("decode({encoded:?}) failed for {original:?}: {e}"),
            };
            assert_eq!(
                recovered, original,
                "roundtrip failed (encoded: {encoded:?})"
            );

            // Anything not passed through is a canonical encoding.
            if encoded != original {
                assert!(encoded.starts_with("_N_"), "{encoded:?} lacks the prefix");
                assert!(is_encoded(&encoded), "{encoded:?} is not canonical");
                assert_eq!(encode(&decode(&encoded).unwrap()), encoded);
            }
        }
    }

    /// `encode_forced` is total and lossless for the same corpus, and always
    /// produces a prefixed form even where `encode` would pass through.
    #[test]
    fn corpus_forced_roundtrip() {
        for original in corpus() {
            let encoded = encode_forced(&original);
            assert!(encoded.starts_with("_N_"), "{encoded:?} lacks the prefix");
            assert!(
                is_xid_identifier(&encoded),
                "encode_forced({original:?}) = {encoded:?} is not a valid identifier"
            );
            assert_eq!(decode(&encoded).unwrap(), original);
        }
    }

    /// Encoding is injective: no two corpus entries share an encoding.
    #[test]
    fn corpus_encodings_are_distinct() {
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for original in corpus() {
            let encoded = encode(&original);
            if let Some(other) = seen.insert(encoded.clone(), original.clone()) {
                panic!("{original:?} and {other:?} both encode to {encoded:?}");
            }
        }
    }

    #[test]
    fn passthrough_is_exactly_non_prefixed_identifiers() {
        for s in [
            "foo",
            "bar123",
            "_private",
            "CamelCase",
            "café",
            "名前",
            "привет",
            "_",
            "__",
            "foo__bar",
            "a___b",
        ] {
            assert_eq!(encode(s), s, "should pass through: {s:?}");
            assert!(!is_encoded(s), "{s:?} should not read as an encoding");
        }

        for s in ["", "123", "foo bar", "foo-bar", "_N_test", "_N_"] {
            assert_ne!(encode(s), s, "should be encoded: {s:?}");
        }
    }

    #[test]
    fn empty_string_has_an_encoding() {
        assert_eq!(encode(""), "_N_");
        assert!(is_xid_identifier("_N_"));
        assert_eq!(decode("_N_").unwrap(), "");
    }

    #[test]
    fn encode_is_not_idempotent_but_is_lossless() {
        // Encoding an encoding encodes it again; the round trip still works.
        let once = encode("hello world");
        let twice = encode(&once);
        assert_ne!(once, twice);
        assert_eq!(decode(&twice).unwrap(), once);
    }

    #[test]
    fn is_xid_identifier_grammar() {
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

    #[test]
    fn decode_error_cases() {
        // No prefix
        assert_eq!(decode("foo"), Err(DecodeError::NotEncoded));
        assert_eq!(decode("hello world"), Err(DecodeError::NotEncoded));
        // '6' is outside the base-32 alphabet
        assert!(matches!(
            decode("_N_abc__6"),
            Err(DecodeError::InvalidDigit('6'))
        ));
        // Uppercase digits are outside it too
        assert!(matches!(
            decode("_N_helloworld__FA0B"),
            Err(DecodeError::InvalidDigit('F'))
        ));
        // 'z' = 25 >= threshold(32, 72) = 1, so more digits are expected
        assert_eq!(decode("_N_abc__z"), Err(DecodeError::UnexpectedEnd));
        // Parses, but is not the canonical spelling of ""
        assert_eq!(decode("_N___"), Err(DecodeError::NonCanonical));
    }

    #[test]
    fn decode_error_display() {
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
        assert_eq!(
            DecodeError::NonCanonical.to_string(),
            "input is not the canonical encoding of its value"
        );
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
            ("", "_N_"),
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
        assert_eq!(encode(""), "_N_");
    }

    // SPEC.md § Collision Handling examples
    #[test]
    fn collision_handling() {
        assert_eq!(encode("_N_test"), "_N__N_test");
        assert_eq!(decode("_N__N_test").unwrap(), "_N_test");

        assert_eq!(encode("foo__bar"), "foo__bar");
        assert_eq!(encode("__"), "__");

        // A literal that is itself a valid encoding is still encoded.
        assert_eq!(encode("_N_helloworld__fa0b"), "_N__N_helloworld_fa0b__oa3l");
        assert_eq!(
            decode("_N__N_helloworld_fa0b__oa3l").unwrap(),
            "_N_helloworld__fa0b"
        );
    }

    // Prefix collision with non-basic characters: _N_ prefix AND non-XID chars
    #[test]
    fn prefix_collision_with_non_basic() {
        let cases = vec![
            "_N_fgyd#",
            "_N_ ",
            "_N_hello world",
            "_N_foo-bar",
            "_N_test!",
            "_N_a@b",
            "_N_#",
            "_N_123#abc",
        ];
        for input in cases {
            let encoded = encode(input);
            assert!(
                encoded.starts_with("_N_"),
                "expected _N_ prefix for {:?}, got {:?}",
                input,
                encoded
            );
            assert_ne!(encoded, input, "should not pass through: {:?}", input);
            let decoded = decode(&encoded).unwrap_or_else(|e| {
                panic!(
                    "decode failed for {:?} (encoded: {:?}): {:?}",
                    input, encoded, e
                )
            });
            assert_eq!(decoded, input, "roundtrip failed for {:?}", input);
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Roundtrip property: decode(encode(s)) == s, with passthrough forms
        /// standing for themselves.
        #[test]
        fn prop_roundtrip(s in ".*") {
            let encoded = encode(&s);
            // Only try to decode if it was actually encoded (has prefix)
            if encoded.starts_with("_N_") {
                let decoded = decode(&encoded).unwrap_or_else(|e| {
                    panic!("decode failed for input '{}' with encoding '{}': {:?}", s, encoded, e)
                });
                prop_assert_eq!(&decoded, &s, "roundtrip failed for: {}", &s);
            } else {
                // If not encoded, the output should equal input (passthrough)
                prop_assert_eq!(&encoded, &s, "passthrough failed for: {}", &s);
            }
        }

        /// `encode_forced` is total and lossless for every input.
        #[test]
        fn prop_forced_roundtrip(s in ".*") {
            let encoded = encode_forced(&s);
            prop_assert!(encoded.starts_with("_N_"));
            prop_assert_eq!(decode(&encoded).unwrap(), s);
        }

        /// Canonicality: for encodings, encode_forced(decode(s)) == s
        #[test]
        fn prop_identity(s in ".*") {
            let encoded = encode(&s);
            // Only test identity for actually encoded strings
            if encoded.starts_with("_N_") {
                let decoded = decode(&encoded).unwrap();
                let re_encoded = encode_forced(&decoded);
                prop_assert_eq!(&re_encoded, &encoded, "identity failed for: {}", &s);
            }
        }

        /// Valid output: encode always produces valid XID identifiers
        #[test]
        fn prop_valid_output(s in ".*") {
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
                    decode(&encoded).unwrap_or_else(|e| panic!("decode failed for {}: {:?}", s, e));
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
                    decode(&encoded).unwrap_or_else(|e| panic!("decode failed for {}: {:?}", s, e));
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
                // Case sensitive: the encoder never emits uppercase
                assert!(result.is_none());
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

    /// Verify the empty string gets the bare-prefix encoding
    #[kani::proof]
    fn verify_encode_empty() {
        assert_eq!(encode(""), "_N_");
        assert_eq!(decode("_N_").unwrap(), "");
    }

    /// Verify decode fails for non-encoded strings
    #[kani::proof]
    fn verify_decode_requires_prefix() {
        // Any string not starting with _N_ should fail
        let result = decode("abc");
        assert!(matches!(result, Err(DecodeError::NotEncoded)));
    }

    /// Verify encoding an encoding is lossless (it is not idempotent)
    #[kani::proof]
    fn verify_double_encode_is_lossless() {
        let input = "a b";
        let once = encode(input);
        let twice = encode(&once);
        assert_ne!(once, twice, "encode should not pass an encoding through");
        assert_eq!(decode(&twice).unwrap(), once);
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
