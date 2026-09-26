# Namecode

Encode arbitrary Unicode strings into valid programming language identifiers.
Think "Punycode for variable names."

Output is a valid [UAX 31](https://unicode.org/reports/tr31/) identifier,
compatible with Rust, Go, JavaScript, and Python.

## Usage

```rust
use namecode::{encode, decode};

// Valid XID identifiers pass through unchanged
assert_eq!(encode("foo"), "foo");
assert_eq!(encode("café"), "café");
assert_eq!(encode("名前"), "名前");

// Non-XID characters get encoded
let encoded = encode("hello world");
assert!(encoded.starts_with("_N_"));
assert_eq!(decode(&encoded).unwrap(), "hello world");

// So does the empty string, and anything that merely looks encoded
assert_eq!(encode(""), "_N_");
assert_eq!(decode(&encode("_N_helloworld__fa0b")).unwrap(), "_N_helloworld__fa0b");
```

## Properties

| Property | Definition |
|----------|------------|
| **Roundtrip** | `decode(encode(s)) == s` whenever `encode(s) != s`; a passthrough stands for itself (`decode` answers `NotEncoded`) |
| **Forced roundtrip** | `decode(encode_forced(s)) == s` for every `s`, and `encode_forced(decode(t)) == t` for every `t` `decode` accepts |
| **Passthrough** | Valid XID identifiers not starting with `_N_` pass through unchanged |
| **Canonical** | Each value has exactly one encoding; `decode` rejects any other spelling |
| **Valid output** | `encode(s)` is always a valid identifier, for every `s` |
| **O(n)** | Linear time encode and decode |

`encode` is deliberately not idempotent: encoding an encoding encodes it
again. Passing an encoding through unchanged would make it impossible to tell
a literal `_N_helloworld__fa0b` from the encoding of `hello world`, and
`decode` would silently corrupt the former. Use `is_encoded` to test whether a
string is already an encoding, and `encode_forced` when you need a
`_N_`-prefixed form even for input that would otherwise pass through.

## CLI

```bash
cargo install namecode

namecode encode "hello world"
# _N_helloworld__fa0b

namecode decode "_N_helloworld__fa0b"
# hello world

echo "foo-bar" | namecode encode
# _N_foobar__da1d
```

## Specification

See [SPEC.md](SPEC.md) for the full encoding format, algorithm details, and
test vectors.

## License

Apache-2.0
