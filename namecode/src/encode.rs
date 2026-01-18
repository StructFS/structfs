//! Namecode encoding implementation.

use crate::bootstring::{adapt_bias, encode_digit, threshold, BASE, INITIAL_BIAS};

/// The prefix marking encoded strings.
pub const PREFIX: &str = "_N_";

/// The delimiter between basic chars and encoded portion.
pub const DELIMITER: &str = "__";

/// Check if a string is a valid XID identifier per UAX 31.
///
/// A valid identifier starts with XID_Start (or underscore followed by XID_Continue)
/// and continues with XID_Continue characters.
pub fn is_xid_identifier(s: &str) -> bool {
    let mut chars = s.chars();

    match chars.next() {
        None => false, // Empty string is not a valid identifier
        Some(first) => {
            if first == '_' {
                // Underscore must be followed by at least one XID_Continue
                match chars.next() {
                    None => false, // Just "_" is not valid
                    Some(second) => {
                        unicode_ident::is_xid_continue(second)
                            && chars.all(unicode_ident::is_xid_continue)
                    }
                }
            } else {
                unicode_ident::is_xid_start(first) && chars.all(unicode_ident::is_xid_continue)
            }
        }
    }
}

/// Check if a string needs encoding.
///
/// A string needs encoding if:
/// - It's not a valid XID identifier, OR
/// - It starts with `_N_` (prefix collision), OR
/// - It contains `__` (delimiter collision)
pub fn needs_encoding(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    // Prefix collision
    if s.starts_with(PREFIX) {
        return true;
    }

    // Delimiter collision
    if s.contains(DELIMITER) {
        return true;
    }

    // Not a valid XID identifier
    !is_xid_identifier(s)
}

/// Encode a Unicode string into a valid UAX 31 identifier.
///
/// Returns input unchanged if already a valid XID identifier that doesn't
/// conflict with our encoding format.
pub fn encode(input: &str) -> String {
    // Empty string passes through
    if input.is_empty() {
        return String::new();
    }

    // Check if encoding is needed
    if !needs_encoding(input) {
        return input.to_string();
    }

    // Check if already encoded - must verify it decodes AND re-encodes to same value
    if input.starts_with(PREFIX) {
        if let Ok(decoded) = crate::decode::decode(input) {
            // Only treat as already-encoded if the decoded value would need encoding
            // and re-encoding produces the exact same result
            if needs_encoding(&decoded) {
                let re_encoded = encode_impl(&decoded);
                if re_encoded == input {
                    return input.to_string();
                }
            }
        }
    }

    encode_impl(input)
}

/// Internal encoding implementation.
pub fn encode_impl(input: &str) -> String {
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
    let mut basic = String::new();
    let mut non_basic: Vec<(usize, char)> = Vec::new();

    for (i, &c) in chars.iter().enumerate() {
        if is_basic[i] {
            basic.push(c);
        } else {
            non_basic.push((i, c));
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

    #[test]
    fn test_is_xid_identifier() {
        // Valid identifiers
        assert!(is_xid_identifier("foo"));
        assert!(is_xid_identifier("_foo"));
        assert!(is_xid_identifier("foo123"));
        assert!(is_xid_identifier("café"));
        assert!(is_xid_identifier("名前"));
        assert!(is_xid_identifier("_1"));

        // Invalid identifiers
        assert!(!is_xid_identifier("")); // Empty
        assert!(!is_xid_identifier("123")); // Starts with digit
        assert!(!is_xid_identifier("foo bar")); // Contains space
        assert!(!is_xid_identifier("foo-bar")); // Contains hyphen
        assert!(!is_xid_identifier("_")); // Just underscore
    }

    #[test]
    fn test_needs_encoding() {
        // Don't need encoding
        assert!(!needs_encoding("foo"));
        assert!(!needs_encoding("café"));
        assert!(!needs_encoding("")); // Empty passes through

        // Need encoding
        assert!(needs_encoding("foo bar")); // Space
        assert!(needs_encoding("foo-bar")); // Hyphen
        assert!(needs_encoding("123foo")); // Starts with digit
        assert!(needs_encoding("_N_test")); // Prefix collision
        assert!(needs_encoding("foo__bar")); // Delimiter collision
    }

    #[test]
    fn test_encode_valid_xid() {
        // Valid XID identifiers pass through unchanged
        assert_eq!(encode("foo"), "foo");
        assert_eq!(encode("café"), "café");
        assert_eq!(encode("名前"), "名前");
        assert_eq!(encode("foo123"), "foo123");
    }

    #[test]
    fn test_encode_empty() {
        assert_eq!(encode(""), "");
    }

    #[test]
    fn test_encode_with_space() {
        let encoded = encode("hello world");
        assert!(encoded.starts_with(PREFIX));
        assert!(encoded.contains(DELIMITER));
        // Basic chars should be extracted
        assert!(encoded.contains("helloworld"));
    }

    #[test]
    fn test_encode_with_hyphen() {
        let encoded = encode("foo-bar");
        assert!(encoded.starts_with(PREFIX));
        assert!(encoded.contains("foobar"));
    }

    #[test]
    fn test_encode_starts_with_digit() {
        let encoded = encode("123foo");
        assert!(encoded.starts_with(PREFIX));
    }

    #[test]
    fn test_encode_prefix_collision() {
        let encoded = encode("_N_test");
        assert!(encoded.starts_with(PREFIX));
        // Should NOT equal the input (would be ambiguous)
        assert_ne!(encoded, "_N_test");
    }

    #[test]
    fn test_encode_delimiter_collision() {
        let encoded = encode("foo__bar");
        assert!(encoded.starts_with(PREFIX));
        // Should NOT equal the input (would be ambiguous with basic/encoded delimiter)
        assert_ne!(encoded, "_N_foo__bar");
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
