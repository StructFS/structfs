# Isotope 2026-09-14 specification candidate

Status: 0.4 implementation candidate; not tagged or published.
Registry evidence is maintained in [release status](../docs/release-status.md).

The normative source is `isotope/spec/`. This snapshot accompanies the StructFS and Featherweight 0.4.0 candidate.
The previous 0.3 packages are published. Isotope is a specification, not a Cargo
package. A future specification tag must identify the reviewed implementation
commit; this work does not create or publish a tag. Site builds copy the normative source; do not
edit generated site chapters independently.

## Independent version domains

| Contract | Candidate |
| --- | --- |
| Isotope specification snapshot | 2026-09-14 |
| StructFS / Featherweight crates | 0.4.0 candidate |
| StructFS Value and canonical tagged JSON | v1 |
| Core-Wasm binding | Spec 11 in this snapshot; exactly `structfs.read` and `structfs.write` imports |
| Optional capability profiles | v1, discovered per provider |

This snapshot does not introduce runtime negotiation of a whole-spec version.
Deployers pin compatible runtime/SDK versions and check required profile versions.
Unknown required profiles must fail explicitly. Additive specification clarification
can retain a profile version; incompatible payload or semantic changes require a
new profile/binding contract and migration notes. A crate version alone does not
prove every optional specification feature is implemented.

## Conformance evidence

| Contract | Executable evidence in this repository |
| --- | --- |
| Paths, composition and namespaces (03) | `packages/core-store` conformance/unit tests; `featherweight/runtime/tests/assembly_integration.rs` |
| Value fidelity and protocol results (06, 11) | `packages/serde-store/tests/value_v1.rs`, independent `reference_value_v1.py`, runtime Wasm echo tests |
| Lifecycle/server ownership (05, 07, 13) | runtime `embedding_resources.rs`, `overload.rs`; service `ownership.rs`; external embedding consumer |
| WASI shim (10) | runtime `wasi_tower.rs` |
| Determinism/replay (12) | runtime `transcript.rs` and browser-host shared fixtures |
| Revisioned state (14) | state `state.rs`, `protocol.rs`; runtime `state_service.rs` |
| Interactive and operation profiles (14) | profiles `interactive.rs`, `operation.rs`; all three external application consumers |

This matrix identifies evidence, not full certification of every normative sentence.
Native release CI covers Linux/macOS at Rust 1.96 and stable. Guest builds target
`wasm32-unknown-unknown`. Browser-host checks execute under Node; cross-engine DOM,
worker residency, Windows adapters, production sockets, durable recovery and OS
process isolation are outside this candidate's certification.

The application fixtures use bounded fake network/process providers and a journal
with explicitly limited recovery guarantees. Read the
[application evidence](../docs/design/2026-09-12-application-profiles-and-fixtures.md)
before transferring those claims to an application.

## 0.4 contract additions and support

Owned recoverable hosting is a native core-Wasm embedding API. The component
adapter implements the full driver context with joined blocking execution; it
and the browser host do not expose the new owned host-state recovery API. No
adapter may claim it implements a policy merely because it accepts the same
Wasm imports. Pin the 2026-09-14 snapshot with 0.4 runtime/SDKs; older handwritten
server responses without explicit presence must be rebuilt or adapted.

Value v1 and existing optional profile versions remain unchanged. Store conventions
apply at exposed interfaces; internal snapshot and mutation representations remain
implementation choices. See [0.4 migration](../docs/migration-0.4.md).
