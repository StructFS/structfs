//! Namecode encoding implementation.

use crate::bootstring::{adapt_bias, encode_digit, threshold, BASE, INITIAL_BIAS};

/// The prefix marking encoded strings.
pub(crate) const PREFIX: &str = "_N_";

/// The delimiter between basic chars and encoded portion.
pub(crate) const DELIMITER: &str = "__";

/// Check if a string is a valid XID identifier per UAX 31.
///
/// A valid identifier starts with XID_Start (or underscore) and continues
/// with XID_Continue characters. Single underscore `_` is valid.
///
/// # Examples
///
/// ```
/// use namecode::is_xid_identifier;
///
/// assert!(is_xid_identifier("foo"));
/// assert!(is_xid_identifier("_private"));
/// assert!(is_xid_identifier("café"));
///
/// assert!(!is_xid_identifier(""));
/// assert!(!is_xid_identifier("foo bar"));
/// assert!(!is_xid_identifier("123abc"));
/// ```
pub fn is_xid_identifier(s: &str) -> bool {
    let mut chars = s.chars();

    match chars.next() {
        None => false, // Empty string is not a valid identifier
        Some(first) => {
            if first == '_' {
                // Underscore alone or followed by XID_Continue is valid
                chars.all(unicode_ident::is_xid_continue)
            } else {
                unicode_ident::is_xid_start(first) && chars.all(unicode_ident::is_xid_continue)
            }
        }
    }
}

/// Check if a string needs encoding.
///
/// A string needs encoding if:
/// - It is empty (the empty string is not an identifier), OR
/// - It's not a valid XID identifier, OR
/// - It starts with `_N_` (prefix collision)
///
/// The `_N_` rule is unconditional: `_N_test` is a perfectly good identifier,
/// but passing it through would make it indistinguishable from the encoding
/// of `test`, and `decode` would then answer `test` for a string that was
/// never encoded. Encoding every `_N_`-prefixed input keeps `encode`
/// injective, and keeps the passthrough set disjoint from the encodings, so
/// `decode` never mistakes a passthrough for an encoding.
///
/// Note: Strings containing `__` do NOT need encoding just because of that.
/// The delimiter `__` only has meaning after the `_N_` prefix, so `foo__bar`
/// passes through unchanged since it can't be confused with an encoded string.
pub(crate) fn needs_encoding(s: &str) -> bool {
    // Prefix collision - only strings starting with _N_ could be confused
    // with encodings. Everything that is not a valid XID identifier (the
    // empty string included) has to be encoded anyway.
    s.starts_with(PREFIX) || !is_xid_identifier(s)
}

/// Encode a Unicode string into a valid UAX 31 identifier.
///
/// Returns input unchanged if it is already a valid XID identifier that
/// cannot be confused with an encoding — that is, one that does not start
/// with `_N_`. Everything else, including the empty string and anything
/// already `_N_`-prefixed, is encoded.
///
/// The output is always a valid XID identifier, and `encode` is injective:
/// when `encode(s) != s`, [`decode`](crate::decode) returns exactly `s`; when
/// `encode(s) == s`, the passthrough stands for itself and `decode` answers
/// `NotEncoded`. `encode` is therefore *not* idempotent — `encode(encode(x))` double-encodes
/// — which is the price of a lossless round trip. Use
/// [`is_encoded`](crate::is_encoded) if you need to know whether a string is
/// already an encoding.
///
/// # Examples
///
/// ```
/// use namecode::{decode, encode};
///
/// // Valid identifiers pass through
/// assert_eq!(encode("foo"), "foo");
/// assert_eq!(encode("café"), "café");
///
/// // Non-identifier characters trigger encoding
/// assert_eq!(encode("hello world"), "_N_helloworld__fa0b");
/// assert_eq!(encode("foo-bar"), "_N_foobar__da1d");
///
/// // The empty string has an encoding of its own
/// assert_eq!(encode(""), "_N_");
/// assert_eq!(decode("_N_").unwrap(), "");
///
/// // Anything that looks like an encoding is encoded again, so the round
/// // trip never loses the original
/// let literal = "_N_helloworld__fa0b";
/// assert_ne!(encode(literal), literal);
/// assert_eq!(decode(&encode(literal)).unwrap(), literal);
/// ```
pub fn encode(input: &str) -> String {
    if needs_encoding(input) {
        encode_impl(input)
    } else {
        input.to_string()
    }
}

/// Encode a string unconditionally, even when it is already a valid
/// identifier that `encode` would pass through.
///
/// The result always starts with `_N_` and always decodes back to `input`.
/// This exists for callers whose identifier grammar is narrower than UAX 31 —
/// StructFS path components, for instance, reject a bare `_` that
/// [`encode`] happily passes through — and that therefore need an escape
/// hatch that is guaranteed to produce a `_N_`-prefixed form.
///
/// # Examples
///
/// ```
/// use namecode::{decode, encode, encode_forced};
///
/// assert_eq!(encode("foo"), "foo");
/// assert_eq!(encode_forced("foo"), "_N_foo");
/// assert_eq!(decode(&encode_forced("foo")).unwrap(), "foo");
/// ```
pub fn encode_forced(input: &str) -> String {
    encode_impl(input)
}

/// Report whether `s` is a canonical Namecode encoding, i.e. whether
/// [`decode`](crate::decode) accepts it.
///
/// # Examples
///
/// ```
/// use namecode::{encode, is_encoded};
///
/// assert!(is_encoded(&encode("hello world")));
/// assert!(!is_encoded("hello"));
/// // Non-canonical spellings are not encodings
/// assert!(!is_encoded("_N_helloworld__FA0B"));
/// assert!(!is_encoded("_N___"));
/// ```
pub fn is_encoded(s: &str) -> bool {
    crate::decode::decode(s).is_ok()
}

/// Internal encoding implementation.
pub(crate) fn encode_impl(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();

    // First pass: identify which characters are basic vs non-basic
    // A character is non-basic if:
    // 1. It's not XID_Continue, OR
    // 2. It's an underscore following another underscore (to avoid __ in basic)
    let mut is_basic: Vec<bool> = vec![true; chars.len()];
    let mut consecutive_underscores = 0;

    for (i, &c) in chars.iter().enumerate() {
        if !unicode_ident::is_xid_continue(c) {
            is_basic[i] = false;
            consecutive_underscores = 0;
        } else if c == '_' {
            consecutive_underscores += 1;
            if consecutive_underscores >= 2 {
                is_basic[i] = false;
            }
        } else {
            consecutive_underscores = 0;
        }
    }

    // Count non-basic characters
    let non_basic_count = is_basic.iter().filter(|&&b| !b).count();

    // If there are non-basic chars, ensure basic doesn't end with underscore
    // (to avoid ambiguity with delimiter __)
    if non_basic_count > 0 {
        // Find the last basic character index
        for i in (0..chars.len()).rev() {
            if is_basic[i] {
                if chars[i] == '_' {
                    is_basic[i] = false;
                } else {
                    break;
                }
            }
        }
    }

    // Build basic string and non-basic list
    // We also need to ensure no consecutive underscores in the final basic string.
    // This can happen when non-consecutive underscores in input become adjacent after
    // removing non-basic characters.
    let mut basic = String::new();
    let mut non_basic: Vec<(usize, char)> = Vec::new();
    let mut last_was_underscore = false;

    for (i, &c) in chars.iter().enumerate() {
        if is_basic[i] {
            // Check if this would create consecutive underscores in basic
            if c == '_' && last_was_underscore {
                // Mark as non-basic to avoid __ in basic
                non_basic.push((i, c));
            } else {
                basic.push(c);
                last_was_underscore = c == '_';
            }
        } else {
            non_basic.push((i, c));
            // Non-basic chars don't affect underscore tracking for basic string
        }
    }

    // If no non-basic chars, we still need the prefix (for prefix collision or digit start)
    if non_basic.is_empty() {
        return format!("{}{}", PREFIX, basic);
    }

    // Encode non-basic chars
    let encoded = encode_insertions(&non_basic);

    format!("{}{}{}{}", PREFIX, basic, DELIMITER, encoded)
}

/// Encode non-basic character insertions.
///
/// Uses a simple encoding: for each insertion, encode position delta and codepoint
/// as variable-length integers using bias adaptation.
fn encode_insertions(insertions: &[(usize, char)]) -> String {
    let mut output = String::new();
    let mut bias: u32 = INITIAL_BIAS;
    let mut prev_pos: usize = 0;

    for (idx, &(pos, c)) in insertions.iter().enumerate() {
        // Encode position delta (from previous position)
        let pos_delta = if idx == 0 { pos } else { pos - prev_pos - 1 };

        encode_varint(&mut output, pos_delta as u32, bias);
        bias = adapt_bias(pos_delta as u32, (idx + 1) as u32, idx == 0);

        // Encode codepoint
        let cp = c as u32;
        encode_varint(&mut output, cp, bias);
        bias = adapt_bias(cp, (idx + 2) as u32, false);

        prev_pos = pos;
    }

    output
}

/// Encode a value as a variable-length integer using bootstring encoding.
fn encode_varint(output: &mut String, mut value: u32, bias: u32) {
    let mut k: u32 = BASE;

    loop {
        let t = threshold(k, bias);

        if value < t {
            output.push(encode_digit(value).expect("value should be < BASE"));
            break;
        }

        let digit = t + (value - t) % (BASE - t);
        output.push(encode_digit(digit).expect("digit should be < BASE"));

        value = (value - t) / (BASE - t);
        k += BASE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Contract-level behaviour (passthrough, round trip, error cases) lives in
    // the crate-root test module. These tests cover the internals: which
    // strings the encoder decides to touch, and the structural invariants of
    // the encoded form.

    #[test]
    fn test_needs_encoding() {
        // Don't need encoding
        assert!(!needs_encoding("foo"));
        assert!(!needs_encoding("café"));
        assert!(!needs_encoding("foo__bar")); // Valid XID, no prefix collision
        assert!(!needs_encoding("_")); // Bare underscore is a valid identifier

        // Need encoding
        assert!(needs_encoding("")); // Empty is not an identifier
        assert!(needs_encoding("foo bar")); // Space
        assert!(needs_encoding("foo-bar")); // Hyphen
        assert!(needs_encoding("123foo")); // Starts with digit
        assert!(needs_encoding("_N_test")); // Prefix collision
        assert!(needs_encoding("_N_foo__bar")); // Prefix collision (__ irrelevant)
        assert!(needs_encoding("_N_helloworld__fa0b")); // Even a real encoding
    }

    #[test]
    fn test_encode_structure() {
        // Basic chars are preserved in order, ahead of the delimiter
        let encoded = encode("hello world");
        assert!(encoded.starts_with(PREFIX));
        assert!(encoded.contains(DELIMITER));
        assert!(encoded.contains("helloworld"));

        // No delimiter when every character is basic
        assert_eq!(encode("123foo"), "_N_123foo");
    }

    #[test]
    fn test_encode_forced_always_prefixes() {
        for input in ["foo", "_", "", "café"] {
            let encoded = encode_forced(input);
            assert!(
                encoded.starts_with(PREFIX),
                "{input:?} -> {encoded:?} lacks prefix"
            );
            assert_eq!(crate::decode::decode(&encoded).unwrap(), input);
        }
    }

    #[test]
    fn test_encode_non_canonical_encoding_is_raw_input() {
        // '9' is not in the bootstring alphabet, so this is not an encoding
        // at all; uppercase digits parse but are non-canonical. Both are
        // treated as ordinary strings that need encoding.
        for input in ["_N_abc__9", "_N_helloworld__FA0B"] {
            let encoded = encode(input);
            assert!(encoded.starts_with(PREFIX));
            assert_ne!(encoded, input);
            assert_eq!(crate::decode::decode(&encoded).unwrap(), input);
        }
    }

    #[test]
    fn test_encode_consecutive_underscores_with_non_basic() {
        // When a string has consecutive underscores AND non-basic chars,
        // the second underscore gets moved to non-basic to avoid __ in basic
        let encoded = encode("a__b c");
        assert!(encoded.starts_with(PREFIX));
        let decoded = crate::decode::decode(&encoded).unwrap();
        assert_eq!(decoded, "a__b c");
    }

    #[test]
    fn test_encode_trailing_underscore() {
        // "_ " should encode without trailing underscore in basic
        let encoded = encode("_ ");
        assert!(encoded.starts_with(PREFIX));
        // Should have delimiter since there are non-basic chars
        assert!(encoded.contains(DELIMITER));
        // The basic part (between _N_ and __) should not end with underscore
        let after_prefix = &encoded[PREFIX.len()..];
        let delim_pos = after_prefix.find(DELIMITER).unwrap();
        let basic = &after_prefix[..delim_pos];
        assert!(
            !basic.ends_with('_'),
            "basic '{}' ends with underscore",
            basic
        );
    }
}
