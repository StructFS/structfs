# Ox supplement follow-up audit — 2026-09-16

This audit closes the evidence gaps identified after the September 15 adoption
revision (`c68b417`). It also fixes two lifecycle defects exposed by reviewing
those cases. The original implementation and its other acceptance evidence are
recorded in [the supplement validation](release-validation-supplement-2026-09-15.md).

## Lifecycle fixes and evidence

`SupervisedProtocol` now hands the opened handle to its registered cleanup using
an awaited one-shot channel. Previously cleanup could block an executor worker
on a mutex held by synchronous open. The new current-thread test closes the owner
while open is paused on a blocking worker, checks that the close deadline remains
runnable, then abandons the late allocation and verifies supervised cleanup.

A shutdown-hook panic previously skipped the producer join. With unwinding
enabled, cleanup now catches that panic, awaits the join callback and reports
failure. The regression verifies resource release and terminal publication before
failure reconciliation. Abort-on-panic builds cannot provide unwind recovery.

The release test now explicitly polls the reader to Pending and polls a release
future to Pending before dropping it. Two store aliases await the same cleanup;
acknowledgment follows resource drop and terminal publication, with exactly one
shutdown request. Existing tests retain coverage for timeout, final-store Drop,
producer failure and supervisor drain. The earlier record described parked reads
and abandoned futures without actually polling those futures to Pending; that
coverage claim was too strong.

## Diagnostics and downstream evidence

The Wasm diagnostic test now exercises both `start_sync` and `start_async` for
read and write imports, using TypeMismatch, ResourceLimit and UnsupportedFormat.
Async hosts deliberately yield before returning to exercise suspension. Serving
envelope round trips remain checked.

Run the optional read-only downstream probe with:

```sh
python3 scripts/check-ox-letter.py /path/to/ox --sse
```

All four unchanged SSE framing tests from Ox commit
`2bcc9e635da08331dc3c13cdd217f852afc2da8f` pass through a test-only StructFS adapter:
every chunk split, UTF-8, multiline data, CRLF/bare CR, EOF, frame limits and
completed events preceding an error. Source SHA-256:
`784d906705a6366eee62587eb6cef571a2213350f2cf45e858c4cc18b49261aa`.
The script retains generated sources, a migration diff and provenance under
`local/`. It does not modify Ox.

This is fixture-level adoption evidence, not a full Ox transport migration.
Ox's framer replaces malformed UTF-8; StructFS rejects it. The test-only adapter
asserts valid UTF-8 at EOF because the unchanged fixtures require it. Production
callers must handle StructFS framing errors, including EOF errors, explicitly.
Provider event interpretation and reconnect policy remain downstream concerns.

## Validation

| Check | Result |
| --- | --- |
| Quality gate | Formatting, strict Clippy and 1,389 tests passed; 24 ignored |
| Coverage | 90.03% regions, 89.49% lines; unchanged 90% region threshold |
| Full release gate | Passed, including all 20 extracted archives, standalone consumers, isolated features, portable graphs, docs and rebuilt guests |
| Browser host | All 21 tests passed |
| Ox source probes | Original subscription/path probe: 28 passed; supplementary SSE probe: 4 passed |

Final gate results and candidate archive hashes are recorded in the accompanying
[manifest](ox-supplement-audit-2026-09-16.json). No packages were published and no
changes were made to the Ox checkout. The persistence, paging and Masked contract
boundaries from the preceding validation remain unchanged.
