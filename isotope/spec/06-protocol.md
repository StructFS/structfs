# Protocol

Isotope routes StructFS reads and writes. The native semantic boundary is:

```text
read(path) -> Result<Option<Value>, Error>
write(path, value) -> Result<Path, Error>
```

`None` is absence. `Some(Null)` is a present value. Null is not a universal delete
command; deletion is a provider operation. Some compatibility stores retain a
historical delete-on-null convention and must document it. Writes may return a
new handle path, which is significant and must pass namespace confinement checks.

Higher-level semantics belong to the stores implementing them. Core read/write
success carries the meaning documented by that store; the core does not prescribe
transactions, persistence, event ordering, streaming or configuration workflows.
A durable store may acknowledge persistence through ordinary write success.
Optional store profiles standardize particular contracts without extending the
core requirements for other stores. Routing and runtime execution preserve the
store's semantics rather than supplying stronger guarantees.

## Value and serialization

[StructFS Value v1](../../docs/specs/structfs-value-v1.md) is normative for values,
conversion, resource limits, canonicalization and fidelity. Its domains are Null,
Bool, signed i64 Integer, u64 Unsigned, IEEE-754 binary64 Float, Unicode String,
Bytes, ordered Array, and unique string-keyed Map. Signedness, arbitrary bytes,
negative zero and non-finite floats are meaningful. Paths are distinct from map
keys; arbitrary map keys must not be concatenated into capability paths.

The direct Serde bridge converts typed application data into this IR. Serde
compatibility does not promise support for every Serde shape or every encoding.
Unsupported shapes and lossy conversion fail with typed codec errors. Never use
a JSON intermediate to translate arbitrary Values.

| Encoding | Reference support |
| --- | --- |
| Tagged StructFS Value JSON v1 | Lossless Value profile, explicitly versioned media type |
| CBOR | Documented Value v1 profile; rejects unsupported keys/tags and duplicate keys |
| FlexBuffers | Documented Value v1 profile and fidelity restrictions |
| Plain JSON | Compatibility subset; cannot faithfully carry all Value domains |
| Protobuf, MessagePack, RON | No universal reference codec promised by this release |

A core guest declares its serialization in its manifest. The assembly may select
an override according to the binding. The reference host validates support before
execution; it never infers support for a format merely from its MIME name. The
selected format remains fixed for that execution. Runtime translation decodes to
Value and re-encodes with the destination codec; unsupported values fail explicitly.
An opaque same-format Record can avoid decoding where the provider supports it.

Schema-specific protocols such as gRPC require an application adapter. Advertising
`application/protobuf` does not create such an adapter.

## Effects, ordering and handles

Reads may consume input, open resources, wait for events, or perform external I/O.
They are not automatically pure, repeatable or safe to retry. Pure snapshots and
read-only profile discovery are explicit provider contracts. A pending async call
parks execution without blocking the shared executor. Cancellation can stop the
wait without rolling back a committed effect.

An operation start may return an owned handle. Its bounded status, result,
cancellation and release operations are specified in [capability profiles](14-capability-profiles.md).
Cancellation requests and joined termination are distinct observations. Dropping a
guest or losing a returned path does not remove the host's cleanup responsibility.

Consistency is provider-specific. Revisioned state supplies atomic batches,
conditional expected tokens, immutable snapshots and observation/resynchronization
through `structfs.state` v1. Binary streams supply consuming reads and bounded
backpressure through `structfs.binary_stream` v1. Neither implies durable storage.

## Errors and bindings

Native errors distinguish NotFound, PermissionDenied, Conflict, Overloaded,
DeadlineExceeded, Cancelled, ResourceLimit, invalid paths and typed codec failures.
Callers inspect these variants, not diagnostic strings. An overload or timeout is
not permission to retry an effect: the caller needs a provider idempotency contract.

The [core-Wasm binding](11-core-wasm-binding.md) carries stable negative status
categories and a UTF-8 diagnostic. It does not preserve every field of a native
codec error. Profile-specific faults requiring richer machine-readable data use
versioned Value reply envelopes, as revisioned state does.

The [server protocol](07-server-protocol.md) carries response Values. Explicit
`present` distinguishes absence from Null; unmarked historical responses retain
their compatibility meaning. Its older error names remain accepted. Cancellation
and resource limits have separate error names. Historical error envelopes lacking
a structured path cannot reconstruct every native path error exactly.

## Discovery and references

`iso/capabilities` enumerates granted mount prefixes without granting authority.
A provider's `meta/profiles` declares independent profile versions and support
levels. Discovery must not probe effectful paths to guess capabilities. Reading
a provider root is not a universal directory listing operation.

Application Values may contain references such as `{path: "users/123"}` and
pagination links. Following them is an ordinary capability-checked read or write;
a string in a Value does not grant access. Returned handles, projected map keys,
and payload paths remain subject to the provider's confinement rules.
