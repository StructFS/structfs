# StructFS Value v1

Date: 2026-09-11

Status: Draft specification; proposed for the next coordinated crate release

Specification identifier: `structfs-value/1`

## 1. Scope and status

This specification defines the StructFS semantic value model, a complete tagged
JSON representation, native JSON/CBOR/FlexBuffers profiles, and direct typed
Serde conversion. It is the detailed value contract for the
[application substrate design](../design/2026-09-11-application-substrate-and-value-ir.md).
It refines that design's sections 5 and 6, including previously deferred
canonical JSON rules. The current code, migration changes, and verification are
tracked in the [implementation report](value-v1-implementation.md).

The intended implementation evolves `structfs_core_store::Value`; it does not
introduce a second competing runtime value model. Existing codec behavior must
be migrated explicitly where it differs from this document.

MUST and MUST NOT state conformance requirements. SHOULD states a recommendation
whose exceptions require documentation. MAY states an optional capability.
Requirements apply to the claimed component: a decoder need not implement a
Serde bridge, and a core value library need not depend on any wire codec.

Application schemas, paths, capabilities, store operations, revision protocols,
stream framing, compression, encryption, and Wasm ABIs are outside this format.
An object resembling a reference or operation remains ordinary data unless an
application explicitly interprets it. Decoding MUST NOT resolve references,
open resources, execute operations, or grant capabilities.

## 2. Abstract value model

A value is a finite, acyclic tree with exactly one of these kinds:

| Kind | Domain |
|---|---|
| Null | One present null value |
| Bool | `false` or `true` |
| Integer | Mathematical integers from `-9223372036854775808` through `18446744073709551615`, inclusive |
| Float | IEEE 754 binary64 values, with all NaNs identified as one value |
| String | A finite sequence of Unicode scalar values, represented as valid UTF-8 |
| Bytes | A finite sequence of octets |
| Array | A finite ordered sequence of values |
| Map | A finite mapping from unique Strings to values |

The root MAY have any kind. Empty strings, bytes, arrays, maps, and map keys are
valid. Strings and keys MAY contain U+0000, controls, or noncharacters. Surrogate
code points are not Unicode scalar values and are invalid. Unicode normalization,
case folding, and filesystem-name normalization MUST NOT be performed.

Map order is not semantic. Key equality compares decoded Unicode scalar
sequences, equivalently their valid UTF-8 bytes. For example, `é` and `e` followed
by U+0301 are distinct keys. Duplicate keys MUST fail before insertion could
overwrite a previous entry. Different source escapes do not make keys distinct.
Map keys need not be valid StructFS Path components.

An absent value is not Null. An API such as `Result<Option<Value>, Error>` uses
its outer `None` to represent absence. Null MUST NOT acquire universal deletion
semantics through this model; a store profile may separately define that operation.

### 2.1 Integer normalization

Signedness and original machine width are not semantic properties. Implementations
with separate signed and unsigned storage variants MUST treat equal nonnegative
integers equally. Normalized Rust storage uses the signed variant when the integer
fits i64 and the unsigned variant otherwise. Wider input integers MUST undergo
exact range checking; conversion through floating point is forbidden.

There is one integer zero. Integer negative zero normalizes to it wherever an
input profile permits that spelling. The tagged JSON profile rejects that spelling.
Arbitrary precision integers and decimal arithmetic are not v1 kinds.

### 2.2 Float normalization

Float and Integer are distinct even when numerically equal. Float positive zero
and negative zero are distinct. Finite values, subnormals, and infinities retain
their binary64 representation. Every NaN sign and payload normalizes to the
quiet NaN bit pattern `7ff8000000000000`.

Normalization MUST inspect bits or use an equivalent operation without depending
on a signaling NaN's arithmetic behavior. No arithmetic interpretation of a Float
is implied by storage. A narrower binary floating input widens exactly before
normalization.

### 2.3 Equality

Semantic equality is an equivalence relation defined as follows:

1. Different kinds are unequal.
2. Null equals Null; Bools, Integers, Strings, and Bytes compare exactly.
3. Floats compare their normalized binary64 bits, including zero sign.
4. Arrays compare length and corresponding elements recursively.
5. Maps compare their sets of keys and corresponding values recursively.

In particular, NaN is semantically equal to NaN. Ordinary IEEE float equality
and a derived Rust `PartialEq` are not a substitute. An implementation SHOULD
expose a clearly named semantic equality operation. If it exposes hashing as
part of an equality-based collection API, equal values MUST hash equally; such
an API's hash is not a persistent content identifier.

## 3. Profiles and selection

The following identifiers name contracts defined here, independently of crate
versions. They are exact, case-sensitive strings when used in a StructFS profile
registry or capability description.

| Profile identifier | Wire representation | Encodable values |
|---|---|---|
| `structfs-value-json/1` | Tagged JSON, section 4 | All v1 values |
| `structfs-json/1` | Plain JSON, section 5 | Values without Bytes or non-finite Floats |
| `structfs-cbor/1` | Native CBOR, section 6 | All v1 values |
| `structfs-flexbuffers/1` | Native FlexBuffers, section 7 | Values without U+0000 in any map key |
| `structfs-serde/1` | In-memory typed bridge, section 8 | Supported Serde shapes |

All claims are subject to explicitly configured resource limits and the underlying
format's representable lengths. A format name alone does not establish fidelity.
In particular, third-party Serde codecs are not automatically these profiles.

The proposed media type for tagged JSON is
`application/vnd.structfs.value+json;version=1`. This is a proposed identifier,
not a claim of IANA registration. Until registration, use an explicit codec/profile
selection under the application's transport contract. Plain JSON and CBOR can
use their existing media types with the profile agreed separately. The current
`application/x-structfs-value` parsed-value hint does not name encoded bytes.

Selection MUST be explicit through an API, negotiated protocol, or declared
configuration. A decoder MUST NOT switch from plain JSON to tagged JSON because
input resembles an envelope. Conflicting declared and embedded versions MUST
fail. Unknown versions MUST fail rather than being interpreted as the latest
supported version. No negotiation or fallback is triggered by parse failure.

Each invocation decodes one bounded document. JSON/CBOR trailing data is rejected
as specified below; FlexBuffers uses the complete supplied buffer. A stream
transport MUST supply framing independently. A value does not contain its
application schema version unless that schema includes one explicitly.

## 4. StructFS Value JSON v1

### 4.1 JSON syntax and document grammar

The underlying syntax is [JSON](https://www.rfc-editor.org/rfc/rfc8259.html).
Input MUST be UTF-8 without a BOM, contain exactly one document, and have no
comments or trailing commas. JSON whitespace is permitted before and after the
document and between tokens. A decoder MUST reject invalid UTF-8 and unpaired
surrogate escapes; paired surrogate escapes decode to their single scalar value.

The following grammar describes JSON values, not a second textual parser:

```text
document = ["structfs-value", 1, node]
node     = ["null"]
         | ["bool", boolean]
         | ["int", decimal-string]
         | ["float", bits-string]
         | ["string", string]
         | ["bytes", base64-string]
         | ["array", [node, ...]]
         | ["map", [[string, node], ...]]
```

Every bracket pair denotes a JSON array. Ellipses mean zero or more elements,
not literal tokens. The envelope has exactly three elements. Its version token
MUST be the JSON number spelled `1`, not `1.0`, `1e0`, or a string. The envelope
name and node tags compare as decoded strings. Every node and map entry has
exactly the shown arity. Native JSON objects and native JSON null have no position
in this grammar. Booleans occur only as the second element of a `bool` node.

Unknown tags, additional fields, missing fields, and mismatched payload types
MUST fail. There is no extension namespace inside v1 nodes. User arrays and maps
are always wrapped, so user data cannot collide with a tag or envelope.

### 4.2 Scalars

`decimal-string` MUST match `0|-?[1-9][0-9]*` over ASCII characters and be within
the Integer range. A plus sign, whitespace, leading zero, and `-0` are invalid.
Range checking MUST precede narrowing to a machine integer.

`bits-string` MUST contain exactly 16 lowercase ASCII hexadecimal digits. It
spells the binary64 bit pattern from most significant nibble to least significant
nibble, independently of host endianness. It is not a decimal float spelling.
Decoders accept any such bit pattern and normalize NaNs. Encoders emit only the
normalized NaN. Examples:

| Meaning | Bits |
|---|---|
| Positive zero | `0000000000000000` |
| Negative zero | `8000000000000000` |
| One | `3ff0000000000000` |
| Smallest positive subnormal | `0000000000000001` |
| Largest finite positive Float | `7fefffffffffffff` |
| Positive infinity | `7ff0000000000000` |
| Negative infinity | `fff0000000000000` |
| Normalized NaN | `7ff8000000000000` |

`base64-string` uses the standard alphabet and padding of
[RFC 4648 section 4](https://www.rfc-editor.org/rfc/rfc4648#section-4).
Its decoded string MUST have no whitespace, use `+` and `/` rather than URL-safe
substitutes, include exactly the necessary trailing `=` padding, and have zero
unused pad bits. Decoders MUST reject noncanonical base64, including `AB==`
where `AA==` is required. The empty string encodes empty Bytes. Base64 rules apply
after JSON string unescaping.

A `string` payload is the decoded JSON string, with no further interpretation.
For example, a String containing `AA==` is never Bytes.

### 4.3 Containers

An `array` payload is an ordered list of nodes. A `map` payload is a list of
two-element entries whose first element is a decoded String key and second is
a node. Duplicate keys are invalid even if their values are semantically equal.

Decoders MUST accept any ordering of unique map entries. Encoders MUST emit keys
in ascending lexicographic order of their decoded UTF-8 bytes, treating each byte
as unsigned and placing a proper prefix before a longer string. Sorting uses
neither locale nor UTF-16 code units nor escaped JSON spellings.

### 4.4 Canonical encoding

The default encoder for this profile MUST produce the following unique byte
representation, denoted `C1(value)`:

1. Normalize the value according to section 2 and wrap it in the v1 envelope.
2. Use exactly the tags, arities, decimal forms, hex forms, and base64 forms above.
3. Sort every map as specified in section 4.3. Preserve array order.
4. Emit no whitespace outside strings, no BOM, and no trailing newline.
5. Emit strings between ASCII double quotes. Escape a double quote as `\"`
   and a backslash as `\\`. Escape U+0000 through U+001F as `\u00xx` with
   lowercase hex digits. Emit every other scalar directly as UTF-8.

Consequently canonical strings do not use short escapes such as `\n`, escape
the solidus, escape DEL, or escape U+2028/U+2029. These rules also apply to keys,
tags, and the envelope name. They define StructFS canonical JSON and MUST NOT
be described as another JSON canonicalization standard.

A normal decoder accepts otherwise legal alternate whitespace and string escapes,
unsorted unique maps, and alternate NaN bits. A canonical-validation mode MUST
reject a document unless its original bytes equal `C1` of its decoded value.
This mode MUST be explicitly selectable; it MUST NOT silently normalize bytes
and report the original document as canonical. Pretty output MAY be offered by
a separately selected presentation API and remains decodable, but is not `C1`.

For all values within limits:

```text
decode(C1(v)) is semantically equal to v
C1(a) == C1(b) if and only if a is semantically equal to b
```

These are byte and semantic identity guarantees, not a digest or signature
protocol. An application introducing hashes MUST separately select its algorithm,
domain separation, schema identity, and version migration. Existing transcript
hashes MUST NOT silently switch to `C1`.

### 4.5 Examples

Each line below is a complete canonical document; displayed line separators are
not part of the document:

```json
["structfs-value",1,["null"]]
["structfs-value",1,["int","18446744073709551615"]]
["structfs-value",1,["float","8000000000000000"]]
["structfs-value",1,["bytes","AP8="]]
["structfs-value",1,["string","line\u000abreak"]]
["structfs-value",1,["array",[["int","1"],["float","3ff0000000000000"]]]]
["structfs-value",1,["map",[["",["null"]],["a\u0000b",["bool",true]],["payload",["bytes",""]]]]]
```

The user Array containing String `structfs-value`, Integer `1`, and an Array
containing String `null` is encoded as:

```json
["structfs-value",1,["array",[["string","structfs-value"],["int","1"],["array",[["string","null"]]]]]]
```

## 5. Plain JSON profile

`structfs-json/1` uses the same UTF-8, document, and Unicode validation rules as
section 4.1, with ordinary JSON values rather than the tagged grammar. Its
mapping is null to Null, boolean to Bool, string to String, array to Array,
and object to Map. Duplicate decoded object keys MUST fail.

A number token without a fraction or exponent decodes to an exact Integer.
`-0` decodes to Integer zero. Tokens containing a fraction or exponent decode
to Float using correctly rounded binary64 conversion, round to nearest with
ties to even. Negative zero sign MUST survive. Decimal underflow MAY round to
a subnormal or signed zero; a token whose rounded result is infinite MUST fail.
Decimal text is not promised exact decimal arithmetic.

Integer overflow MUST fail even if the token could be approximated as a Float.
For example, `18446744073709551615` succeeds exactly, and `18446744073709551616`
fails. Conversely `1e0` and `1.0` are Floats, not Integers. Parsing through a DOM
that already rounded integer tokens is not conforming.

An encoder MUST reject any Bytes or non-finite Float anywhere in a value. It
MUST NOT replace them with strings, arrays, or Null. Integer output is exact
decimal. Float output MUST be a decimal token that decodes to the same Float
bits and contains a fraction or exponent. For example, encode Float one as
`1.0`, not `1`, and negative zero as `-0.0`. Map output MUST use section 4.3's
key order. Other JSON spelling choices are not canonicalized by this profile.

This profile preserves every accepted value's kinds and normalized values on
an encode/decode round trip. It does not promise interoperability with clients
that represent every JSON number as a JavaScript Number. Such clients need an
explicit safe-integer schema restriction or the tagged profile.

## 6. CBOR profile

`structfs-cbor/1` uses [RFC 8949 CBOR](https://www.rfc-editor.org/rfc/rfc8949.html)
with the following StructFS restrictions. A document MUST contain exactly one
item and no trailing octets. Only definite-length strings and containers are
accepted. This restriction is a v1 profile decision, not a limitation of CBOR.

| CBOR item | StructFS interpretation |
|---|---|
| Major type 0 | Integer, full u64 range |
| Major type 1 | Integer, restricted to the v1 lower bound |
| Major type 2 | Bytes |
| Major type 3 | Valid UTF-8 String |
| Major type 4 | Array |
| Major type 5 | Map, text-string keys only |
| Simple false, true, null | Bool, Bool, Null |
| Half/single/double float | Float, exact widening and NaN normalization |

All tags, including bignums and self-described CBOR, are rejected. Undefined,
other simple values, break markers, invalid/reserved encodings, invalid UTF-8,
and non-text map keys are rejected. Duplicate decoded keys are rejected before
collapse. Encoded argument `n` in major type 1 means `-1-n`; `n` greater than
`9223372036854775807` is out of range.

Decoders MUST accept valid wider-than-necessary argument encodings, any order
of unique map keys, and all three float widths. Writers MUST use the shortest
integer/length argument width, definite lengths, map order from section 4.3,
and binary64 for every Float, including normalized NaN. Bool and Null use their
single-byte encodings. Float width and original integer width are not preserved.
These writer rules are deterministic but are not an assertion of RFC 8949 core
deterministic encoding, whose preferred float encoding differs.

Representative complete documents, in hexadecimal:

| Value | Writer output |
|---|---|
| Null | `f6` |
| Integer -1 | `20` |
| Minimum Integer | `3b7fffffffffffffff` |
| Maximum Integer | `1bffffffffffffffff` |
| Float negative zero | `fb8000000000000000` |
| Float NaN | `fb7ff8000000000000` |
| Bytes `00 ff` | `4200ff` |
| String `a` | `6161` |
| Map `a` to Null | `a16161f6` |

## 7. FlexBuffers profile

`structfs-flexbuffers/1` uses the complete bounded buffer as one FlexBuffers
document. Decoders MUST validate the root, type and width metadata, lengths,
offset arithmetic, referenced ranges, string/key termination, UTF-8, and map
key ordering before using unsafe or coercing accessors. A valid reference must
meet the underlying format's offset rules. Unknown/reserved type codes fail.

| FlexBuffers item | StructFS interpretation |
|---|---|
| Null, Bool | Null, Bool |
| Int/UInt, including indirect forms | Exact Integer |
| Float, including indirect forms | Float, exact widening and NaN normalization |
| String | String |
| Key in value position | String, subject to key termination rules |
| Blob | Bytes |
| Untyped, typed, and fixed typed vectors | Array, with each element interpreted by its declared type |
| Map | Map with unique String keys |

The accepted type codes are 0 through 14, 16 through 26, and 36, with the meanings
in the [upstream type definitions](https://raw.githubusercontent.com/google/flatbuffers/master/rust/flexbuffers/src/flexbuffer_type.rs)
and the table above. All are required. Code 15 (deprecated string vectors) and
all other codes MUST fail as unsupported; adding an upstream code does not
automatically extend this profile. Integer payload widths are 1, 2, 4, or 8 octets;
float payload widths are 4 or 8 octets. Bool payloads MUST encode 0 or 1.
No numeric accessor may
silently coerce a string, Bool, Float, or missing value into an Integer.
[The FlexBuffers documentation](https://flatbuffers.dev/flexbuffers/) distinguishes
type inspection from accessors that perform conversions, and distinguishes blobs
from typed vectors. A vector of u8 is an Array of Integers, not Bytes.

FlexBuffers keys are NUL-terminated. A native encoder MUST reject a Map key
containing U+0000 before calling a builder; it MUST NOT truncate the key or panic.
Ordinary String values are length-delimited and MAY contain U+0000. This limitation
is also explicitly documented by the upstream Rust
[`MapBuilder`](https://raw.githubusercontent.com/google/flatbuffers/master/rust/flexbuffers/src/builder/map.rs)
and was checked in the locally resolved dependency, version 25.12.19.
It makes this profile a strict subset of the v1 model.

Encoders MUST preserve kind, exact Integer value, Float zero sign, and normalized
NaN. They MAY choose valid inline/indirect forms, exact narrower floats, shared
storage, and typed vectors. Map keys MUST have the ordering required by the
underlying format. No canonical FlexBuffers byte encoding is defined here.

Buffer aliasing MUST NOT create semantic identity between values. Each logical
occurrence counts toward expanded-tree limits, even if its bytes are shared.
Invalid offsets and structures fail; expansion MUST NOT recurse indefinitely.
Padding or unused storage allowed by the underlying format is not part of the
semantic value and need not survive transcoding. A FlexBuffers buffer has no
StructFS envelope; the caller's profile selection is required.

## 8. Direct Serde bridge

### 8.1 Contract

`structfs-serde/1` specifies a Serde `Serializer` producing Value and a Serde
`Deserializer` consuming Value. Proposed entry points are `to_value` and
`from_value`; API spelling is not part of the wire contract. They MUST operate
directly on the IR, without a JSON intermediate.

Both sides MUST report `is_human_readable() == true`. This is an explicit choice
for the stable structural mapping; it MUST NOT vary with an eventual wire codec.
A custom type may therefore choose its documented human-readable representation.
An alternate binary-oriented bridge would need a separate profile identifier.

The bridge maps the [Serde data model](https://serde.rs/data-model.html), not
arbitrary Rust memory or type identity. Custom serializers, adapters, and derive
attributes determine the operations the bridge actually receives. A typed round
trip is promised only when the type's serialization and deserialization agree
with this mapping and all conversions succeed.

### 8.2 Serialization

| Serde operation | Result or constraint |
|---|---|
| bool | Bool |
| i8–i128, u8–u128 | Exact normalized Integer; reject outside v1 range |
| f32, f64 | Float; widen f32 exactly, normalize NaN |
| str, char | String; char contributes exactly one scalar |
| bytes | Bytes |
| sequence, tuple, tuple struct | Array |
| map | Map; keys must serialize as strings |
| struct | Map from emitted field names to values |
| unit, unit struct | Null |
| newtype struct | Inner value, without a name wrapper |
| None | Null |
| Some | Non-null inner value; null inner value fails |
| unit variant | String containing variant name |
| newtype variant | One-entry Map from variant name to inner value |
| tuple variant | One-entry Map from variant name to Array of fields |
| struct variant | One-entry Map from variant name to Map of emitted fields |

Map-key serialization accepts `serialize_str` and transparent newtype delegation
to it. Other key shapes, including numbers, bools, chars, units, and unit variants,
MUST fail; callers may explicitly adapt them to strings. Key conversion MUST NOT
be inferred from Display or Debug. Duplicate emitted field names or map keys fail.
Malformed custom serializer calls, such as a value without a key, MUST return an
error rather than panic. Declared known sequence/map lengths MUST match completed
output. Unknown sequence/map lengths MAY be accepted subject to limits.

The `Some` restriction prevents `Some(())` and `Some(None)` from silently becoming
None. It applies after inner serialization, including custom null-producing types.
It does not reject `Some` of an Array or Map merely because a child is Null.

`Vec<u8>` normally uses sequence operations and therefore produces an Array.
Bytes requires a byte-oriented type or adapter. Tuple arity, struct names,
newtype names, enum type names, and original numeric widths are not retained.

### 8.3 Deserialization

Typed requests MUST check kinds and bounds without implicit textual, numeric,
or byte/array coercions:

| Requested shape | Accepted Value |
|---|---|
| bool | Bool |
| signed/unsigned integer | Integer within the destination type's range |
| f64 | Float |
| f32 | Float exactly representable as f32, including signed zeros and infinities; NaN yields normalized f32 quiet NaN |
| string | String |
| char | String with exactly one Unicode scalar |
| bytes/byte buffer | Bytes |
| sequence | Array |
| tuple/tuple struct | Array with exactly the requested arity |
| map/struct | Map; field requirements are enforced by the target visitor |
| unit/unit struct | Null |
| newtype struct | Delegate the same Value to the inner visitor |
| option | Null invokes None; every other kind invokes Some on the same Value |
| enum | The externally tagged shape described below |

Finite f32 narrowing MUST fail on overflow, underflow to a different value, or
rounding to a different value; widening the result must recover the original
binary64 bits. Integers are not automatically accepted for floats or vice versa.
Callers wanting such conversions must request an explicit application adapter.

For externally tagged enums, a String names a unit variant. A one-entry Map names
a payload variant and supplies its payload according to the serialization table.
A unit variant MUST NOT also accept a Map containing Null. A newtype variant
whose inner type is unit can use that Map, so variant payload shape is preserved.
Unknown variant names and payload shape mismatches fail through the target type.

`deserialize_any` visits normalized Integers as i64 where possible and u64
otherwise; Float as f64; and other kinds through their corresponding visitors.
Ignored fields MAY be skipped semantically, but skipping MUST NOT bypass wire
validation or resource limits. Borrowed deserialization MAY be supplied when the
IR lifetime permits it; owned deserialization MUST be supported.

Flattening and internally/adjacently tagged or untagged enums are supported only
to the extent that Serde's emitted operations and target visitors satisfy these
rules. The release MUST publish tested examples and unsupported cases. Unknown
struct fields follow the destination schema's Serde policy, rather than a global
reject-unknown-fields rule for ordinary user Maps.

### 8.4 Explicit option representation

An opt-in `ExplicitOption<T>` adapter MUST serialize None as this Map:

```json
{"kind":"none"}
```

It MUST serialize Some as a Map with exactly `kind` equal to String `some` and
`value` equal to the inner representation, including Null:

```json
{"kind":"some","value":null}
```

These examples use ordinary JSON to display IR Maps, not tagged-wire syntax.
Adapter deserialization MUST reject extra keys, missing keys, unknown kinds, and
non-string kinds. It MUST NOT auto-detect these Maps when deserializing an ordinary
Option. Each ambiguous nested option level requires its own adapter. For example,
Some of explicit None is `{"kind":"some","value":{"kind":"none"}}`.
Using this adapter changes an application's schema and requires its migration.

### 8.5 Value serialization versus codec serialization

`Serialize` and `Deserialize` implementations on Value SHOULD expose the
structural mapping above, making direct IR conversion an identity modulo
normalization. They are distinct from the tagged JSON codec. Passing Value to an
arbitrary JSON serializer MUST NOT be documented as lossless: that serializer
may choose its own handling of bytes, floats, or numbers.

Supported codec entry points MUST enforce their profile independently, including
duplicate detection and type checks. Generic Serde support that loses information
before those checks is insufficient. `ron::Value` is not an alternate IR bridge:
[RON documents limitations on Value-mediated round trips](https://github.com/ron-rs/ron#limitations).
A future RON authoring frontend requires a typed schema or explicit lowering rules;
comments and source metadata are outside this value model.

## 9. Resource and error contracts

### 9.1 Bounded processing

Every public untrusted-input decode entry point MUST accept or apply documented,
finite limits. A caller MUST be able to select stricter limits. Limits MUST cover:

| Limit | Counting rule |
|---|---|
| Input/output bytes | Complete encoded document size in octets |
| Value depth | Root depth 0; each Array element or Map value adds 1 |
| Value nodes | Root plus every recursively occurring value; keys are not nodes |
| Collection entries | Elements of each Array or entries of each Map |
| String/key bytes | Decoded UTF-8 bytes of each String or key |
| Blob bytes | Decoded octets of each Bytes value |
| Aggregate payload bytes | Sum of all decoded strings, keys, and blobs by occurrence |
| Allocation/work budget | Parser temporaries, token growth, sorting, duplicate tracking, and expanded structures |
| Diagnostic bytes | Maximum returned diagnostic text and location detail |

Limits are inclusive: a count equal to its maximum is allowed. Host allocation
accounting and work units are implementation-specific and MUST be documented;
they are not portable semantic properties. This specification sets no universal
document-size minimum. Protocols requiring interoperability at a certain size
MUST declare compatible limits themselves.

Value-depth accounting excludes tagged JSON wrapper arrays and map entry pairs;
a parser also needs a syntax-depth/work bound so malformed wrappers cannot exhaust
its stack before value validation. Scalar lexical tokens, including leading
whitespace and invalid numeric/base64 tokens, remain subject to byte/work limits.
Checked arithmetic MUST precede allocation and offset calculation. Shared storage
is counted per logical occurrence for the semantic limits.

Encode entry points MUST apply output and traversal limits, including during
canonical map sorting. Direct typed conversion MUST apply analogous tree and
allocation limits. Failure MUST NOT return a truncated value as success.

An encoder writing to a streaming sink MAY have emitted a prefix before failure;
it MUST report failure, and the caller MUST discard that incomplete document.
An API claiming atomic output must buffer or provide a transactional sink.

### 9.2 Error classification

Errors MUST carry a machine-readable category independently of their human
message. The following semantic categories are required; Rust names may differ:

| Category | Meaning |
|---|---|
| `unsupported_profile` | Selected codec/profile is not implemented |
| `unsupported_version` | Well-formed envelope has an unrecognized integer version |
| `syntax` | Invalid underlying syntax, truncated input, or trailing data |
| `invalid_unicode` | Invalid UTF-8 or invalid surrogate encoding |
| `invalid_node` | Invalid envelope/node shape, tag, arity, or scalar lexical form |
| `invalid_base64` | Invalid alphabet, padding, or pad bits |
| `duplicate_key` | Repeated decoded Map key |
| `out_of_range` | Integer or requested numeric conversion outside allowed range |
| `unsupported_value` | Valid source feature or IR value excluded by the selected profile |
| `type_mismatch` | Typed destination cannot consume the supplied kind/shape |
| `ambiguous_option` | Some serializes to Null in the structural bridge |
| `noncanonical` | Otherwise valid tagged document fails canonical validation |
| `resource_limit` | A configured processing bound is exceeded |
| `io` | Source/sink transport failure |

An envelope with numeric integer token `2` yields `unsupported_version`; `1.0`
or a string version yields `invalid_node`. Unknown v1 node tags yield
`invalid_node`. Plain JSON non-finite encoding and FlexBuffers NUL-key encoding
yield `unsupported_value`. Typed exact f32 narrowing failure yields `out_of_range`.

Implementations MAY return whichever applicable error they detect first on input
with multiple faults. Canonical validation MUST first establish a valid semantic
document; it MUST NOT classify malformed base64 or duplicate keys merely as
noncanonical. Resource exhaustion MAY terminate that validation first.

Errors SHOULD include a bounded byte offset and a logical location when available.
Logical locations use typed Array-index and Map-key segments, not an assumed
valid StructFS Path string. No error format may require copying an unbounded key
or source payload into its message. Errors MUST NOT rely on panics for expected
invalid input. Diagnostics are not stable protocol text.

## 10. Record and transcoding

`Record::Raw { bytes, format }` preserves supplied bytes and their declared format
identity. `Record::Parsed(Value)` retains semantics. These are separate contracts:
parsing does not retain whitespace, source escapes, source float/integer widths,
map order, comments, or unused binary storage.

A raw forwarding operation MAY preserve bytes without validating them, but MUST
NOT describe that operation as successful semantic decoding or canonicalization.
A semantic transcode MUST decode under the selected source profile, validate,
then encode under the target profile. Target-subset failures MUST be explicit;
there is no default lossy transcode. A same-format byte-copy optimization cannot
stand in for a caller's requested validation or canonicalization.

For source decoder `D` and target encoder `E`, successful transcoding promises
semantic equality between `D(source)` and the target decoder's result from
`E(D(source))`. It makes no source-byte round-trip promise. Raw identity is the
appropriate contract when original signatures, formatting, or opaque formats
must survive.

## 11. Conformance and release acceptance

An implementation MUST identify the profiles and optional modes it supports,
their effective limits, and any additional platform size restrictions. It MUST
NOT claim a full profile while silently weakening a required numeric, Unicode,
duplicate-key, or type-fidelity rule.

The [companion vectors](fixtures/structfs-value-v1.json) are normative examples
for tagged JSON, plain JSON, and CBOR. They are not an exhaustive test suite.
Their JSON string fields contain the exact document bytes after unescaping;
hex fields contain the exact binary documents. No newline is implied.

Before publishing the migration target, acceptance MUST include:

1. All companion vectors, including rejection and canonicalization cases.
2. Generated bounded value trees round-tripped through every applicable profile
   and compared with semantic equality, including cross-codec transcodes.
3. All integer boundaries; f32 and f64 subnormals; both zeros; infinities; several
   NaN signs/payloads; and exact versus inexact typed narrowing.
4. Invalid UTF-8, lone surrogates, controls, embedded NUL, Unicode-equivalent but
   distinct keys, escaped duplicate keys, and UTF-8 versus UTF-16 ordering cases.
5. Malformed lengths, offsets, type codes, truncated documents, unknown versions,
   invalid arities, and every declared resource limit at and just beyond its bound.
6. Serde fixtures for enums, option ambiguity and explicit nesting, byte adapters,
   flattening, duplicate fields, custom serializers, and human-readable behavior.
7. At least two independently implemented tagged JSON encoders agreeing on `C1`
   for the shared corpus, including a consumer outside the main Rust codec.
8. Fuzz decoding and canonicalization without panics, unchecked allocation,
   infinite recursion, or silent changes in kind or numeric value.
9. Ox/Horns fixtures preserving u64 IDs/revisions, optional state, tagged Views,
   bytes, present Null, and errors through direct typed and guest/host boundaries.

Package tests MUST exercise the final published Cargo archives and enabled codec
features. This document and its examples alone do not satisfy those release gates.

## 12. Adoption and version evolution

The implementation work should proceed in this order:

1. Extend the existing core Value with exact unsigned-range support and semantic
   normalization/equality. Keep wire dependencies out of the core model.
2. Add bounded direct Serde conversion, explicit options, and checked errors.
3. Implement tagged JSON plus canonical validation and run the shared corpus.
4. Enforce the native JSON/CBOR/FlexBuffers profiles and publish the support matrix.
5. Migrate SDK/host boundaries and application fixtures to explicit profile
   selection, then run the coordinated archive release gates.

The current JSON-mediated helpers and legacy byte/base64 behavior are migration
inputs, not alternative interpretations of v1. Where existing stored data lacks
profile identity, migration MUST use an application-declared legacy decoder; it
MUST NOT infer whether an ordinary String originally meant Bytes. Do not rename
existing data as v1 without validating or rewriting it.

Adding a new node kind, changing Integer bounds, changing normalization/equality,
changing tagged grammar, or changing `C1` bytes requires a new model or encoding
version as applicable. Clarifying a requirement without changing accepted values
or canonical bytes may be an editorial revision. Native-profile restrictions and
the Serde mapping are versioned independently; changing them requires new profile
identifiers. Application schema changes remain application responsibilities.

First-class enums, general-key maps, decimal/big integers, timestamps, references,
capabilities, source comments, compression, tagged FlexBuffers fallback, and a
content-addressing protocol are deferred. Applications can represent domain types
with explicitly versioned ordinary Maps today; codecs MUST NOT reserve user keys
for future automatic reinterpretation.
