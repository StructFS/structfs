# StructFS Value v1 implementation and migration

Date: 2026-09-11

The [Value v1 specification](structfs-value-v1.md) is implemented in the existing
core and Serde crates. The shared router is now implemented in the
[P0-B service layer](../design/2026-09-11-shared-async-services.md). Revisioned state
providers, subscriptions, and further service profiles remain separate work.
Nothing is published by
the verification commands below.

## Entry points

`structfs_core_store::Value` adds `Unsigned(u64)`, `From<u64>`, recursive
`normalize()`, and `semantic_eq()`. Ordinary `PartialEq` retains its previous
Rust behavior, so use `semantic_eq` for semantic state comparison. Public enum
constructors can still construct unnormalized values; codecs normalize their
meaning and output.

`structfs_serde_store` exports:

| API | Contract |
|---|---|
| `to_value`, `from_value` | Direct checked structural Serde bridge |
| `to_value_with_limits`, `from_value_with_limits` | Same bridge with caller bounds |
| `ExplicitOption<T>` | Opt-in exact `kind`/`value` option representation |
| `ValueJsonCodec` | Default-limits canonical tagged JSON |
| `JsonCodec`, `CborCodec`, `FlexbuffersCodec` | Default-limits native v1 profiles |
| `ValueCodec::new(Profile::…)` | Explicit profile with configurable `Limits` |
| `ValueCodec::canonical()` | Require canonical tagged JSON on decode |
| `transcode` | Decode, validate, and encode, including same-profile conversions |
| `validate_value` | Check an existing tree against bounds |

`MultiCodec::standard()` includes all four codecs. `Format::VALUE_JSON` selects
the tagged representation; `Format::VALUE` remains an in-memory hint. No content
sniffing selects a codec. `Error::Codec` now includes `CodecErrorKind`, whose
`as_str()` returns the specification's machine-readable category. Unsupported
format selection retains the existing `Error::UnsupportedFormat` variant.

```rust
use structfs_serde_store::{Codec, Format, Profile, Value, ValueCodec};

let value = Value::Array(vec![
    Value::from(u64::MAX),
    Value::Bytes(vec![0, 255]),
    Value::Null,
]);
let codec = ValueCodec::new(Profile::ValueJson).canonical();
let bytes = codec.encode(&value, &Format::VALUE_JSON).unwrap();
let decoded = codec.decode(&bytes, &Format::VALUE_JSON).unwrap();
assert!(value.semantic_eq(&decoded));
```

## Migration changes

`value_to_json` now returns `Result<serde_json::Value, Error>` and rejects Bytes
and non-finite Floats. HTTP body construction propagates those errors. The REPL
uses tagged JSON to display values outside the plain-JSON subset. Large unsigned
integers remain exact. `json_to_value` imports an already parsed, trusted DOM;
it cannot restore duplicate keys or numeric token spellings previously lost by
another parser. Use `JsonCodec` to validate untrusted JSON bytes.

Default typed conversion no longer accepts implicit integer/float conversions,
non-string map keys, or ambiguous `Some(())`/`Some(None)`. Byte-oriented adapters
produce Bytes, while `Vec<u8>` remains an Array. Choose `ExplicitOption` when a
schema must represent a null-valued Some, and version that application schema.

Plain JSON no longer silently maps binary values to base64 strings or NaNs to
null. CBOR rejects tags, indefinite lengths, and non-text keys. FlexBuffers
rejects embedded NUL map keys and deprecated string-vector type 15. Existing
stored data using the earlier conventions needs an explicitly selected legacy
decoder or application migration. Native media types alone do not label old
data as conforming v1 data.

Raw Record forwarding still preserves bytes without validating them. Use
`transcode` when validation is required. Persistent digests retain their existing
protocol; canonical value bytes do not automatically replace transcript hashing.

## Bounds and implementation choices

Defaults are 16 MiB input/output, depth 64, 262,144 value nodes, 65,536 entries per
collection, 4 MiB per string/key, 8 MiB per blob, 16 MiB aggregate payload,
64 MiB allocation reservation, 128 Mi work units, and 256 diagnostic bytes.
`Limits` exposes each bound. Map keys count toward payload bytes but not nodes;
the root has depth zero.

All recursive traversal also has an implementation ceiling of 256 value levels.
JSON syntax has a separate ceiling of 256 levels, including tagged wrappers, so
deep tagged documents may reach that ceiling before a caller's higher semantic
depth bound. These restrictions apply even when a caller raises the defaults.

Allocation accounting is conservative reservation, not exact resident memory:
128 bytes per syntax/value node, twice decoded payload sizes, and additional
reservations for parser strings, encoded output, and FlexBuffers builder stacks
and buffers. Work counts input/output and payload bytes, traversals, and a
conservative key-comparison estimate. Encoding and decoding have separate budgets;
canonical validation and transcoding run both phases. Bounds include library
processing, not arbitrary work or allocations inside application-written Serde
implementations. Callers remain responsible for those implementations.

Diagnostics currently carry the category and bounded text, without source offsets
or logical key/index locations. Borrowed public typed conversion and streaming
sink APIs are not provided; the owned and complete-document APIs are supported.
The serializers report `is_human_readable() == true` independently of wire format.

## Tested Serde support

The conformance tests exercise signed/unsigned primitives through 128-bit checked
inputs, exact f32 conversion, Unicode char/string, units, newtypes, tuples and tuple
structs, maps, structs, externally tagged payload variants, internally tagged
struct variants, adjacently tagged variants, simple untagged alternatives,
flattened maps, byte adapters, explicit nested options, and bounded `collect_str`.

Malformed custom serialization, duplicate keys, wrong lengths, numeric overflow,
inexact narrowing, and mismatched destination kinds fail. Arbitrary custom type
round trips are not promised. In particular, Serde's own buffering for derive
attributes can have its own coercion or ambiguity rules; the bridge cannot
recover type information not emitted by that type.

## Verification

The packaged conformance corpus contains the specification's 88 vectors. Tests
also exercise 1,000 generated trees across the lossless profiles, malformed and
mutated binary buffers, 4,096 deterministic fuzz inputs across all four profiles,
limit boundaries, Serde shape fixtures, and checked transcoding. An independent
Python C1 encoder agrees on the 30 accepted tagged-JSON vectors.

The Featherweight host/guest echo test now includes tagged values with maximum
u64, Bytes, present Null, negative zero, and NaN. The guest SDK's opt-in
`value-codecs` feature exposes `sdk::read_value` and `sdk::write_value` with an
explicit `ValueCodec`. Hosts must configure that same profile and the guest's
manifest must declare its format. Existing raw SDK calls and the reference
guest's JSON manifest retain their existing contracts.

Run:

```sh
cargo test --workspace --all-features --locked --offline
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
python3 packages/serde-store/tests/reference_value_v1.py
scripts/check-featherweight-release.sh
```

The archive gate packages the actual crates, runs the core/Serde conformance tests
and Python encoder from extracted archives, exercises an external consumer, and
builds the guest's codec feature for `wasm32-unknown-unknown`. The HTTP suite needs
normal macOS system-configuration access; a restricted sandbox can fail during
HTTP client initialization before those tests reach their assertions.
