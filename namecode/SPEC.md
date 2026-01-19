# Namecode Specification

**Version:** 1.0
**Status:** Stable

## Abstract

Namecode encodes arbitrary Unicode strings into valid programming language identifiers. Think "Punycode for variable names." The output is valid across Rust, Go, JavaScript, and Python.

## Motivation

Programming languages restrict what characters can appear in identifiers. When storing or transmitting data that uses identifiers as keys (file paths, JSON keys, database columns), arbitrary Unicode strings must be encoded into valid identifier form.

Namecode solves this by providing a reversible, deterministic encoding that:
- Produces valid UAX 31 identifiers
- Preserves readability for ASCII-only inputs
- Handles all Unicode strings including emoji, CJK, RTL text

## Properties

### Guaranteed Properties

| Property | Definition |
|----------|------------|
| **Roundtrip** | If `encode(s)` starts with `_N_`, then `decode(encode(s)) == s` |
| **Passthrough** | If `encode(s)` does not start with `_N_`, then `encode(s) == s` |
| **Identity** | `encode(decode(s)) == s` for all valid encodings `s` |
| **Idempotency** | `encode(encode(s)) == encode(s)` |
| **Valid Output** | `encode(s)` is a valid UAX 31 identifier (for non-empty `s`) |
| **Deterministic** | Same input always produces same output |
| **O(n) Complexity** | Both encode and decode run in linear time |

Note: Strings that are already valid XID identifiers (and don't conflict with the encoding format) pass through unchanged. The `decode` function only accepts strings with the `_N_` prefix.

### Non-Goals

- **Normalization:** Namecode preserves exact codepoints. NFC/NFKC normalization is the caller's responsibility.
- **Minimal Output:** The encoding prioritizes correctness and simplicity over minimal length.
- **Human Readability of Encoded Portion:** The bootstring-encoded section is not meant to be human-readable.

## Terminology

| Term | Definition |
|------|------------|
| **XID Identifier** | A string valid per Unicode Standard Annex #31 (UAX 31). Starts with `XID_Start` or underscore, then zero or more `XID_Continue`. Single underscore `_` is valid. |
| **Basic Character** | A character that passes through unchanged: any `XID_Continue` character except when it would create `__` or a trailing underscore before the delimiter. |
| **Non-Basic Character** | Any character that must be encoded: non-`XID_Continue` characters, consecutive underscores, or trailing underscores when non-basic characters exist. |
| **Prefix** | `_N_` - marks a string as Namecode-encoded. |
| **Delimiter** | `__` - separates basic characters from the encoded portion. |

## Encoding Format

### Grammar

```
namecode    = passthrough | encoded
passthrough = xid_identifier   ; if no collisions
encoded     = "_N_" basic "__" insertions
            | "_N_" basic                      ; no non-basic chars

basic       = { xid_continue } ; no "__", no trailing "_" if insertions exist
insertions  = { position_delta codepoint }
```

### Decision Tree

```
Is input empty?
  └─ Yes → return ""
  └─ No  ↓

Is input a valid XID identifier AND doesn't start with "_N_" AND doesn't contain "__"?
  └─ Yes → return input unchanged (passthrough)
  └─ No  ↓

Is input already a valid Namecode encoding?
  └─ Yes → return input unchanged (idempotency)
  └─ No  ↓

Encode the input:
  1. Extract basic characters (XID_Continue, avoiding __ and trailing _)
  2. Record positions and codepoints of non-basic characters
  3. Encode insertions using Bootstring
  4. Return "_N_" + basic + "__" + encoded_insertions
```

### Examples

| Input | Output | Reason |
|-------|--------|--------|
| `foo` | `foo` | Valid XID, passthrough |
| `cafe` | `cafe` | Valid XID (ASCII) |
| `café` | `café` | Valid XID (Latin extended) |
| `名前` | `名前` | Valid XID (CJK) |
| `foo__bar` | `foo__bar` | Valid XID (contains `__` but no `_N_` prefix) |
| `hello world` | `_N_helloworld__fa0b` | Space is non-XID |
| `foo-bar` | `_N_foobar__da1d` | Hyphen is non-XID |
| `123foo` | `_N_123foo` | Digit can't start identifier (all chars XID_Continue) |
| `_N_test` | `_N__N_test` | Prefix collision (all chars XID_Continue) |
| `_` | `_` | Single underscore is valid XID |
| `` (empty) | `` (empty) | Empty passthrough |

## Bootstring Encoding

The encoded portion uses a variant of the Bootstring algorithm (RFC 3492), adapted for identifier-safe output.

### Alphabet

32 characters, all valid in identifiers:
```
a-z (0-25) + 0-5 (26-31) = 32 characters
```

Characters `6-9` are NOT used (reserved for future extensions).

### Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `BASE` | 32 | Number of distinct digits |
| `T_MIN` | 1 | Minimum threshold |
| `T_MAX` | 26 | Maximum threshold |
| `SKEW` | 38 | Bias adaptation skew |
| `DAMP` | 700 | First-time damping factor |
| `INITIAL_BIAS` | 72 | Starting bias value |

### Variable-Length Integer Encoding

Each value is encoded as a sequence of digits. The threshold function determines when the sequence terminates:

```
threshold(k, bias) =
    T_MIN           if k <= bias + T_MIN
    T_MAX           if k >= bias + T_MAX
    k - bias        otherwise
```

**Encoding a value:**
```
k = BASE
while true:
    t = threshold(k, bias)
    if value < t:
        output encode_digit(value)
        break
    digit = t + (value - t) % (BASE - t)
    output encode_digit(digit)
    value = (value - t) / (BASE - t)
    k += BASE
```

**Decoding a value:**
```
result = 0, w = 1, k = BASE
while true:
    digit = decode_digit(next_char())
    t = threshold(k, bias)
    result += digit * w
    if digit < t:
        break
    w *= (BASE - t)
    k += BASE
```

### Bias Adaptation

After encoding/decoding each value, the bias is adapted:

```
adapt_bias(delta, num_points, first_time):
    delta = delta / DAMP   if first_time else delta / 2
    delta += delta / num_points
    k = 0
    while delta > ((BASE - T_MIN) * T_MAX) / 2:
        delta /= (BASE - T_MIN)
        k += BASE
    return k + ((BASE - T_MIN + 1) * delta) / (delta + SKEW)
```

### Insertion Encoding

Non-basic characters are encoded as a sequence of (position_delta, codepoint) pairs:

1. **Position Delta:** Distance from previous insertion (or from start for first)
   - First insertion: absolute position
   - Subsequent insertions: `current_position - previous_position - 1`

2. **Codepoint:** The Unicode codepoint value

Both are encoded as variable-length integers with bias adaptation between each value.

## Collision Handling

### Prefix Collision (`_N_...`)

Strings starting with `_N_` are always encoded, even if otherwise valid XID. This prevents ambiguity between literal `_N_test` and an encoded string.

```
encode("_N_test") → "_N__N_test" (not "_N_test")
decode("_N__N_test") → "_N_test"
```

Note: If a string starting with `_N_` happens to be a valid Namecode encoding of some other string, the idempotency check will return it unchanged. This is intentional for the encode-encode idempotency property.

### Double Underscore Passthrough

Strings containing `__` but NOT starting with `_N_` pass through unchanged:

```
encode("foo__bar") → "foo__bar" (valid XID, passes through)
encode("__") → "__" (valid XID: underscore followed by XID_Continue)
```

The `__` delimiter only has meaning after the `_N_` prefix, so these strings cannot be confused with encoded strings.

### Basic Portion Constraints

When constructing the basic portion during encoding:

1. **No trailing underscores:** If non-basic characters exist, trailing underscores in basic are moved to non-basic to avoid `___` ambiguity with the delimiter.

2. **No consecutive underscores:** Underscores that would become consecutive in the basic portion (after removing non-basic characters) are moved to non-basic.

```
encode("test_ ") → "_N_test__..." (trailing _ encoded with space)
encode("__ _x") → "_N__x__..." (middle _ encoded to avoid __ in basic)
```

## Error Handling

Decoding can fail with these errors:

| Error | Cause |
|-------|-------|
| `NotEncoded` | Input doesn't start with `_N_` |
| `InvalidDigit(char)` | Character in encoded portion not in alphabet |
| `UnexpectedEnd` | Encoded data truncated mid-varint |
| `InvalidCodepoint(u32)` | Decoded codepoint not valid Unicode |
| `Overflow` | Arithmetic overflow during decoding |

## API

### Rust

```rust
/// Encode a Unicode string into a valid UAX 31 identifier.
pub fn encode(input: &str) -> String;

/// Decode a Namecode string back to Unicode.
pub fn decode(input: &str) -> Result<String, DecodeError>;

/// Quick check if a string appears to be Namecode-encoded.
pub fn is_encoded(input: &str) -> bool;

/// Check if a string is a valid XID identifier (UAX 31).
pub fn is_xid_identifier(input: &str) -> bool;
```

### Command Line

```bash
# Encode a string
namecode encode "hello world"
# Output: _N_helloworld__fa0b

# Decode a string
namecode decode "_N_helloworld__fa0b"
# Output: hello world

# Pipe mode
echo "foo-bar" | namecode encode
cat encoded.txt | namecode decode
```

## Compatibility

### Language Support

Namecode output is valid in:

| Language | Identifier Rules | Namecode Compatible |
|----------|-----------------|---------------------|
| Rust | UAX 31 | Yes |
| Go | UAX 31 (subset) | Yes |
| Python 3 | UAX 31 | Yes |
| JavaScript | UAX 31 | Yes |
| C/C++ | ASCII + some Unicode | Mostly (ASCII subset always works) |

### Version Compatibility

The encoding format is stable. Any string encoded with Namecode 1.0 will decode correctly with future versions.

Future versions may add:
- Alternative prefixes for different use cases
- Extended digit alphabet (6-9 currently reserved)
- Compression optimizations (backward compatible)

## Test Vectors

### Passthrough Cases

| Input | Output |
|-------|--------|
| `foo` | `foo` |
| `_private` | `_private` |
| `café` | `café` |
| `名前` | `名前` |
| `CamelCase` | `CamelCase` |

### Encoding Cases

| Input | Encoded | Notes |
|-------|---------|-------|
| `hello world` | `_N_helloworld__fa0b` | Single space at position 5 |
| `foo-bar` | `_N_foobar__da1d` | Single hyphen at position 3 |
| `a b c` | `_N_abc__ba0bb0b` | Spaces at positions 1 and 3 |
| `123` | `_N_123` | Digits are XID_Continue, no non-basic chars |
| `   ` | `_N___a0ba0ba0b` | Three spaces (all non-basic) |

### Edge Cases

| Input | Output | Notes |
|-------|--------|-------|
| `` | `` | Empty string |
| ` ` | `_N___a0b` | Single space (non-basic) |
| `a` | `a` | Single letter |
| `_` | `_` | Single underscore (valid XID) |
| `_a` | `_a` | Underscore + letter (valid XID) |
| `__` | `__` | Two underscores (valid XID: `_` + XID_Continue) |
| `___` | `___` | Three underscores (valid XID) |
| `foo__bar` | `foo__bar` | Valid XID, passes through |
| `_N_test` | `_N__N_test` | Prefix collision, no non-basic chars |
| `__ _x` | `_N__x__ba3la0ba3l` | Mixed: underscores separated by space |

## Security Considerations

- **No Injection:** Namecode output contains only identifier-safe characters
- **No Normalization Attacks:** Exact codepoints are preserved (no NFKC confusion)
- **Bounded Output:** Output length is O(n) where n is input length
- **Deterministic:** No randomness means no oracle attacks

## References

1. [Unicode Standard Annex #31: Unicode Identifier and Pattern Syntax](https://unicode.org/reports/tr31/)
2. [RFC 3492: Punycode](https://tools.ietf.org/html/rfc3492)
3. [RFC 3454: Stringprep](https://tools.ietf.org/html/rfc3454)
