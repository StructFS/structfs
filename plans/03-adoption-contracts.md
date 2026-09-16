# Adoption contracts after the Ox supplement

Status: implemented and validated for the unpublished 0.4 candidate. Publication remains separate.

The September 15 supplement identifies additional contracts beyond the first
letter. No backward compatibility constraint applies; Null writes remain the
exposed deletion convention, not a universal constraint on internal data.

## Sequence and acceptance

- [x] Joined HandleStore release integrates existing service ownership. Explicit
  release waits; abandoned waits and final Drop retain supervised cleanup.
  Cover parked reads, repeated/concurrent release, failure and supervisor drain.
- [x] Persistence separates buffered acknowledgement from synchronized storage,
  stages memory until save succeeds, and fences ambiguous errors until recovery.
  Cover injected failures, reopen, partial JSONL tails and acknowledgement order.
- [x] LazyRecord serializes fallible initialization; masking scope is explicit.
- [x] Native async HTTP exposes response head and incremental bytes. Independent
  bounded SSE framing preserves completed frames before a later error.
- [x] Child-name discovery has a bounded read projection composable through
  existing async/service adapters without fetching child records.
- [x] Codec-limit failures retain machine-readable classification across guest
  and serving boundaries; document diagnostic reductions precisely.
- [x] Typed OwnedTail guidance covers atomic batches, acknowledgement ownership,
  producer backpressure, terminal status and encoded response-page limits.
- [x] Focused regressions, quality/release gates, migration/spec updates and fresh
  evidence pass; commit the completed revision.

## Verified starting points

Read current `packages/handles/src/handle_store.rs`: close is synchronous and
release discards the entry before invoking it. `structfs-service` already depends
on handles, so integration belongs above handles rather than creating a cycle.
Read `packages/json_store/src/{persist,append_log}.rs`: file writes do not sync;
BackedStore mutates memory before save. Read `packages/core-store/src/lazy_record.rs`:
fallible decode happens before OnceLock::set and may run concurrently. Read
`packages/http/src/executor.rs`: current executor buffers the complete response.
Read core async traits and service envelopes: child enumeration is not forwarded.
Read runtime status/protocol and guest SDK: codec detail is reduced to text.

See [supplement validation](../docs/release-validation-supplement-2026-09-15.md)
for the final quality, package, guest, browser and site evidence.
