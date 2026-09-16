# 0.4 adoption supplement validation — 2026-09-15

Follow-up: the [September 16 audit](ox-supplement-audit-2026-09-16.md) corrects
the lifecycle coverage claims below and records the subsequent fixes and checks.

The [supplement plan](../plans/03-adoption-contracts.md) is implemented and
validated. [Design and migration boundaries](design/2026-09-15-adoption-contracts.md)
explain the final contracts. This record supersedes the original-letter validation
for the current candidate; no package publication or tagging was performed.

Validation used baseline `ef02a92` plus the changes identified by SHA-256 in the
[source and archive manifest](release-validation-supplement-2026-09-15.json).
The manifest identifies the candidate bytes validated before committing them.

| Check | Result |
| --- | --- |
| Workspace quality gate | Passed: formatting, strict Clippy, 1,387 tests; 24 ignored |
| Coverage | 90.02% regions; 89.47% lines; unchanged 90% region threshold |
| Full release gate | Passed: workspace tests/docs, isolated features, portable graphs, extracted packages, rebuilt guests and browser host |
| Final archive gate | Passed again after package README finalization; all 20 final archive hashes recorded |
| Streaming HTTP | Real loopback head-before-body, query/header forwarding, disconnect, bounded error body and truncated transport tests passed |
| Isotope site | Rebuilt normative sources; 19 files generated |
| Browser host | 21 tests passed, including cross-host replay |
| Standalone consumers | Embedding, conversation, reactive screen, streaming gateway and portable consumer checks passed; site Wasm graph checked |

The archive gate now includes JSON-store and guest tests, all-feature HTTP tests,
and the executable typed-tail example in addition to its existing consumers.
Native HTTP checks require ordinary macOS networking/SystemConfiguration access;
the site used locked dependency installation. These checks did not publish or
contact maintainers. Original downstream Ox subscription evidence remains in the
[original-letter audit](ox-letter-audit-2026-09-15.md); this supplement does not
claim that Ox has migrated to the new APIs.

## Acceptance evidence

- `service/tests/handle_cleanup.rs`: parked reads, abandoned release futures,
  repeated/concurrent release, producer-resource drop before acknowledgement,
  cleanup timeout, final store Drop, supervisor drain and visible producer failure.
- JSON-store failure tests: injected write/file-sync/rename/directory-sync errors,
  old in-memory visibility, ambiguous new disk visibility, fenced retries and
  explicit recovery. JSONL rejects incomplete tails without modifying them.
- `json_store/tests/process_reopen.rs`: synchronized snapshot and log data reopen
  after the writer process exits without running Rust destructors.
- HTTP SSE tests: arbitrary chunk/UTF-8 splits, line endings, multiline data,
  metadata, EOF, bounded frames and preservation of events preceding a later error.
- Core/service discovery tests: paged names without record materialization,
  missing versus leaf/array results, response limits, service mounting and cancellation.
- `runtime/tests/codec_diagnostics.rs`: TypeMismatch, ResourceLimit and
  UnsupportedFormat across actual Wasm read/write imports and serving envelopes.
  The Rust SDK tests structured detail and plain-text fallback.
- Core concurrency test: eight simultaneous LazyRecord callers perform one
  successful decode; failed initialization remains retryable.
- Tail tests/example: atomic batch rejection, bounded payload pages, two-reader
  acknowledgement, producer backpressure, terminal status and bounded encoding.

## Deliberate boundaries

Buffered persistence acknowledges OS writes, not power-loss durability. Synced
mode relies on the filesystem honoring synchronization and requires an existing
parent directory. Injected failures and process termination are tested; physical
power loss, all filesystems and concurrent writers are not certified.

Cleanup callbacks must join accepted work; noncooperative producers remain owned
until they finish. Failures require explicit reconciliation. Generic child-page
fallbacks bound responses but materialize names; large providers must override
paging. Masked remains a direct-path lens rather than a subtree-security boundary.
These limitations are explicit contracts, not implied guarantees.
