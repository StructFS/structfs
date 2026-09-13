# Migrating to the coordinated 0.2.0 candidate

0.2.0 is the candidate version in the manifests, not a statement of registry
availability. If an existing 0.2.x release already exposes the older contracts,
these breaking changes require a new incompatible version before publication.

## Values and persisted data

Handle `Value::Unsigned` in exhaustive matches. Use `semantic_eq()` when comparing
semantic state; Rust `PartialEq` retains its prior floating-point behavior.
Use `to_value`/`from_value` for checked direct Serde conversion. Numeric narrowing,
implicit integer/float conversion and non-string map keys fail explicitly.
`Vec<u8>` is an Array; byte-oriented Serde adapters produce Bytes.

Propagate the result of `value_to_json(value)` with `?` or handle the codec error.
Plain JSON cannot preserve Bytes or non-finite floats. Select `ValueJsonCodec`
(tagged JSON), or another explicitly supported lossless profile, for those values.
Use `ExplicitOption<T>` for schemas that distinguish a null-valued Some from None;
version the stored application schema when changing that representation.

Old bytes-to-base64 or NaN-to-null conventions are not automatically migrated.
Inventory persisted data and select a legacy decoder explicitly before rewriting
it with the new codec. Raw Record forwarding does not validate the bytes; use
`transcode` when validation is required. See [Value v1](specs/value-v1-implementation.md)
for the supported shapes, limits and incompatible native codec inputs.

## Service and runtime ownership

Keep registration/operation owners alive for the intended provider lifetime.
Cancellation requests do not prove that native work stopped. Inspect shutdown
reports with `complete()` and retain the cleanup supervisor while work remains;
release resource reservations only after that work is joined. Never automatically
retry an effectful open/start after its result was lost.

Handle admission errors from `deliver_signal` and `deliver_timer`. Assembly wiring
must be an array when supplied. Public request APIs own correlation and cleanup;
replace direct unowned queue writes with those APIs.

## Guest/server responses

Explicit response presence preserves `Some(Value::Null)` separately from absence.
Replace server calls that used `ok_value(Null)` to mean absence with `ok_absent()`.
Unmarked legacy Null responses retain their older interpretation.
Prefer typed SDK calls when callers need ABI error categories and diagnostics;
legacy string-error wrappers remain available. Host and guest must select the same
codec profile, with that format declared in the guest manifest.

## State and profiles

State revision tokens include epochs: reject stale epochs and resnapshot after
expired history. Events remain occurrences even when their payloads compare equal.
Discovery is an optional provider contract, not a requirement on arbitrary stores.
A profile declaration does not add durability, process isolation or transactions;
those guarantees belong to the implementing store.

## Build and acceptance

Use Rust 1.96 or newer. Select only the facade features needed by your application:
`serde`, `json`, `http`, `sys`, `async`, `service`, `state`, or `profiles`; defaults
remain core-only. Guest schemas use their documented no-default-feature modes.

Exercise your actual application's cancellation, remount, persisted-data,
approval and recovery paths against candidate archives. The four independent
consumers under `tests/` demonstrate portable contracts; they do not certify a
production HTTP adapter, database, process sandbox or UI renderer.
