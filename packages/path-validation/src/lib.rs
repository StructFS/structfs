//! The canonical StructFS path component grammar.
//!
//! This crate exists so that everything that validates path components —
//! `structfs-core-store` at runtime and `structfs-path-macro` at compile
//! time — shares one implementation. A forked grammar can drift; a shared
//! one cannot.
//!
//! # Grammar
//!
//! A path component is one of:
//!
//! - A **pure numeric string** (`0`, `42`) — used for array indexing.
//! - A **Unicode identifier** per UAX#31: first char is `XID_Start`, or an
//!   underscore followed by at least one `XID_Continue` char; remaining
//!   chars are `XID_Continue`.
//!
//! Empty components, bare `_`, and components containing `/`, `-`, spaces,
//! or other punctuation are invalid. Arbitrary strings can be made valid
//! with Namecode encoding (see `PathComponent::encode` in core-store).

/// Validate a single path component against the StructFS grammar.
///
/// Returns a human-readable description of the problem on failure.
pub fn validate_component(component: &str) -> Result<(), String> {
    if component.is_empty() {
        return Err("empty component".to_string());
    }

    // Allow pure numeric strings (for array indexing)
    if component.chars().all(|c| c.is_ascii_digit()) {
        return Ok(());
    }

    let mut chars = component.chars();
    let first = chars.next().unwrap();

    // First char: XID_Start or underscore followed by XID_Continue
    let valid_start = unicode_ident::is_xid_start(first)
        || (first == '_'
            && chars
                .clone()
                .next()
                .is_some_and(unicode_ident::is_xid_continue));

    if !valid_start {
        return Err("must start with a letter or underscore followed by letter/digit".to_string());
    }

    // Rest: XID_Continue
    for c in chars {
        if !unicode_ident::is_xid_continue(c) {
            return Err(format!("invalid character '{}' in identifier", c));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_components() {
        for s in ["foo", "_foo", "café", "名前", "0", "42", "a1", "_1"] {
            assert!(validate_component(s).is_ok(), "expected valid: {s:?}");
        }
    }

    #[test]
    fn invalid_components() {
        for s in ["", "_", "-", "a-b", "a b", ".hidden", "1abc", "a/b", "a$b"] {
            assert!(validate_component(s).is_err(), "expected invalid: {s:?}");
        }
    }
}
