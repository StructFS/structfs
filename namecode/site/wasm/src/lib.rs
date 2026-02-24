use wasm_bindgen::prelude::*;

/// Encode a Unicode string into a valid UAX 31 identifier.
#[wasm_bindgen]
pub fn encode(input: &str) -> String {
    namecode::encode(input)
}

/// Decode a namecode-encoded string back to Unicode.
/// Returns the decoded string, or an error message prefixed with "Error: ".
#[wasm_bindgen]
pub fn decode(input: &str) -> String {
    match namecode::decode(input) {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

/// Check if a string is a valid XID identifier (UAX 31).
#[wasm_bindgen]
pub fn is_xid_identifier(input: &str) -> bool {
    namecode::is_xid_identifier(input)
}
