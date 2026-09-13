# Isotope 2026-09-12 specification candidate

Status: frozen candidate for review; not tagged or published.

The normative source is `isotope/spec/`. This snapshot accompanies the StructFS
and Featherweight 0.2.0 candidates. Isotope is a specification, not a Cargo package.
Release it with the annotated tag `isotope/spec-2026-09-12` on the same reviewed
commit as the implementing crates. Site builds copy the normative source; do not
edit generated site chapters independently.

## Independent version domains

| Contract | Candidate |
| --- | --- |
| Isotope specification snapshot | 2026-09-12 |
| StructFS / Featherweight crates | 0.2.0, subject to exact registry preflight |
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
