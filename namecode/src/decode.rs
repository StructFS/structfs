//! Namecode decoding implementation.

use crate::bootstring::{adapt_bias, decode_digit, threshold, BASE, INITIAL_BIAS};
use crate::encode::{DELIMITER, PREFIX};
use crate::DecodeError;

/// Decode a Namecode string back to Unicode.
///
/// Accepts exactly the *canonical* encodings — the strings [`encode_forced`]
/// can produce — and nothing else. Any other input is rejected:
///
/// - `Err(NotEncoded)` if the input does not have the `_N_` prefix, or has
///   the prefix but is structurally not an encoding at all.
/// - `Err(InvalidDigit)` for a character outside the base-32 alphabet. The
///   alphabet is lowercase-only, so `_N_helloworld__FA0B` is rejected even
///   though `_N_helloworld__fa0b` decodes fine: a value must not have two
///   spellings.
/// - `Err(NonCanonical)` if the input parses but re-encoding the result does
///   not reproduce it byte for byte (a redundant delimiter, say).
///
/// Rejecting non-canonical spellings is what makes the forced encoding a
/// bijection: every value has exactly one encoding, so
/// `encode_forced(decode(t)) == t` for every `t` this function accepts.
/// (`encode(decode(t))` may differ from `t`: `decode("_N_foo")` is `"foo"`,
/// which `encode` passes through unchanged.)
///
/// [`encode_forced`]: crate::encode_forced
///
/// # Examples
///
/// ```
/// use namecode::{decode, DecodeError};
///
/// assert_eq!(decode("_N_helloworld__fa0b").unwrap(), "hello world");
/// assert_eq!(decode("_N_foobar__da1d").unwrap(), "foo-bar");
/// assert_eq!(decode("_N_").unwrap(), "");
///
/// // Strings without the _N_ prefix are not valid encodings
/// assert_eq!(decode("foo"), Err(DecodeError::NotEncoded));
/// // Neither are non-canonical spellings of a valid one
/// assert_eq!(decode("_N_helloworld__FA0B"), Err(DecodeError::InvalidDigit('F')));
/// assert_eq!(decode("_N___"), Err(DecodeError::NonCanonical));
/// ```
pub fn decode(input: &str) -> Result<String, DecodeError> {
    let decoded = decode_raw(input)?;

    // Canonicality: an encoding is only valid if it is *the* encoding of the
    // value it decodes to. Without this check the decoder would accept
    // several spellings per value and `encode` would not be injective.
    if crate::encode::encode_forced(&decoded) != input {
        return Err(DecodeError::NonCanonical);
    }

    Ok(decoded)
}

/// Parse an encoding without checking that it is the canonical one.
fn decode_raw(input: &str) -> Result<String, DecodeError> {
    // Check for prefix
    if !input.starts_with(PREFIX) {
        return Err(DecodeError::NotEncoded);
    }

    let without_prefix = &input[PREFIX.len()..];

    // Check if there's a delimiter
    if let Some(delim_pos) = without_prefix.find(DELIMITER) {
        let basic = &without_prefix[..delim_pos];
        let encoded = &without_prefix[delim_pos + DELIMITER.len()..];

        // Decode the insertions
        let insertions = decode_insertions(encoded)?;

        // Reconstruct the original string
        reconstruct(basic, &insertions)
    } else {
        // No delimiter - just basic chars (encoded because of prefix collision,
        // a digit start, or the empty string). All characters must be
        // XID_Continue since encode_impl only puts XID_Continue characters in
        // the basic portion.
        if !without_prefix.chars().all(unicode_ident::is_xid_continue) {
            return Err(DecodeError::NotEncoded);
        }
        Ok(without_prefix.to_string())
    }
}

/// Decode the encoded insertions.
///
/// Each insertion is encoded as: position_delta (varint), codepoint (varint)
fn decode_insertions(encoded: &str) -> Result<Vec<(usize, char)>, DecodeError> {
    if encoded.is_empty() {
        return Ok(Vec::new());
    }

    let mut insertions: Vec<(usize, char)> = Vec::new();
    let mut chars = encoded.chars().peekable();
    let mut bias: u32 = INITIAL_BIAS;
    let mut prev_pos: usize = 0;
    let mut idx: usize = 0;

    while chars.peek().is_some() {
        // Decode position delta
        let pos_delta = decode_varint(&mut chars, bias)?;
        bias = adapt_bias(pos_delta, (idx + 1) as u32, idx == 0);

        // Calculate actual position
        let pos = if idx == 0 {
            pos_delta as usize
        } else {
            prev_pos
                .checked_add(1)
                .and_then(|p| p.checked_add(pos_delta as usize))
                .ok_or(DecodeError::Overflow)?
        };

        // Decode codepoint
        let cp = decode_varint(&mut chars, bias)?;
        bias = adapt_bias(cp, (idx + 2) as u32, false);

        let c = char::from_u32(cp).ok_or(DecodeError::InvalidCodepoint(cp))?;
        insertions.push((pos, c));

        prev_pos = pos;
        idx += 1;
    }

    Ok(insertions)
}

/// Decode a variable-length integer from the character iterator.
fn decode_varint(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    bias: u32,
) -> Result<u32, DecodeError> {
    let mut result: u32 = 0;
    let mut w: u32 = 1;
    let mut k: u32 = BASE;

    loop {
        let c = chars.next().ok_or(DecodeError::UnexpectedEnd)?;
        let digit = decode_digit(c).ok_or(DecodeError::InvalidDigit(c))?;

        let t = threshold(k, bias);

        // result += digit * w
        result = result
            .checked_add(digit.checked_mul(w).ok_or(DecodeError::Overflow)?)
            .ok_or(DecodeError::Overflow)?;

        if digit < t {
            break;
        }

        // w *= (BASE - t)
        w = w.checked_mul(BASE - t).ok_or(DecodeError::Overflow)?;
        k = k.checked_add(BASE).ok_or(DecodeError::Overflow)?;
    }

    Ok(result)
}

/// Reconstruct the original string from basic chars and insertions.
fn reconstruct(basic: &str, insertions: &[(usize, char)]) -> Result<String, DecodeError> {
    let basic_chars: Vec<char> = basic.chars().collect();
    let total_len = basic_chars.len() + insertions.len();

    let mut result = String::with_capacity(total_len * 4); // Estimate for Unicode
    let mut basic_idx = 0;
    let mut insert_idx = 0;

    for pos in 0..total_len {
        if insert_idx < insertions.len() && insertions[insert_idx].0 == pos {
            result.push(insertions[insert_idx].1);
            insert_idx += 1;
        } else if basic_idx < basic_chars.len() {
            result.push(basic_chars[basic_idx]);
            basic_idx += 1;
        } else {
            return Err(DecodeError::Overflow);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Round-trip and contract behaviour live in the crate-root test module;
    // these cover decoder internals and rejection of malformed input.

    #[test]
    fn test_decode_structural_forms() {
        // No prefix at all
        assert_eq!(decode("foo"), Err(DecodeError::NotEncoded));
        // Prefix, no delimiter: the whole tail is basic
        assert_eq!(decode("_N_foo"), Ok("foo".to_string()));
        // Bare prefix is the encoding of the empty string
        assert_eq!(decode("_N_"), Ok(String::new()));
    }

    #[test]
    fn test_decode_rejects_non_xid_basic() {
        // A '-' can never appear in a basic portion the encoder produced.
        assert_eq!(decode("_N_a-b"), Err(DecodeError::NotEncoded));
    }

    #[test]
    fn test_decode_rejects_non_canonical() {
        // Parses to "" (empty basic, empty insertion list) but the canonical
        // encoding of "" is "_N_", not "_N___".
        assert_eq!(decode_raw("_N___"), Ok(String::new()));
        assert_eq!(decode("_N___"), Err(DecodeError::NonCanonical));
    }

    #[test]
    fn test_decode_rejects_uppercase_digits() {
        // The encoder only ever emits lowercase digits, so the uppercase
        // spelling of a real encoding is not itself an encoding.
        assert_eq!(decode("_N_helloworld__fa0b"), Ok("hello world".to_string()));
        assert_eq!(
            decode("_N_helloworld__FA0B"),
            Err(DecodeError::InvalidDigit('F'))
        );
    }

    #[test]
    fn test_reconstruct_overflow() {
        // Test the overflow error in reconstruct when positions are malformed
        // Create a case where insertion position is beyond the expected range
        let basic = "ab";
        // Insertion at position 5 when total_len = 2 + 1 = 3
        let insertions = vec![(5, ' ')];
        let result = reconstruct(basic, &insertions);
        assert_eq!(result, Err(crate::DecodeError::Overflow));
    }

    #[test]
    fn test_reconstruct_overlapping_positions() {
        // Test when insertions have the same position (causes basic to run out)
        let basic = "ab";
        // Two insertions at position 0 - after handling first, second won't match
        // and basic will run out at position 2
        let insertions = vec![(0, ' '), (0, '-')];
        let result = reconstruct(basic, &insertions);
        // Position 2 won't match insertion (0), and basic is exhausted
        assert_eq!(result, Err(crate::DecodeError::Overflow));
    }
}
