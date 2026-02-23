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
```

## Properties

| Property | Definition |
|----------|------------|
| **Roundtrip** | `decode(encode(s)) == s` for all encoded strings |
| **Passthrough** | Valid XID identifiers pass through unchanged |
| **Idempotent** | `encode(encode(x)) == encode(x)` |
| **Identity** | `encode(decode(s)) == s` for valid encodings |
| **O(n)** | Linear time encode and decode |

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
