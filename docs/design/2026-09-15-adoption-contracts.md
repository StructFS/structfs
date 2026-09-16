# Adoption contracts from the Ox supplement

This revision addresses the September 15 supplement within the unpublished 0.4
candidate. It builds on the first letter's work; publication remains separate.

## Handle release

`HandleStore` makes a released handle inaccessible immediately and requests
`HandleProtocol::close` once. The Null-write future then awaits `close_wait`.
Repeated/concurrent releases wait for the same cleanup. Abandoning a future does
not undo release. Pending or failed releases retain tombstones; successful waits
remove them. Protocols can expose `close_complete` so the next operation reaps
completed abandoned tombstones. The conservative default does not infer completion.

`structfs_service::SupervisedProtocol` provides the concrete ownership integration.
It reserves a cleanup registration before invoking the inner protocol's open,
uses that registration's cancellation token for parked reads, and runs inner
close followed by the supplied producer-join callback under the existing owner.
An awaited channel hands the opened handle to cleanup, so closing the owner
during synchronous open does not block an executor worker. A panic in inner close
still runs the join callback before reporting failure (with unwinding enabled).
The callback must await accepted work and publish terminal status. The configured
wait timeout returns an error while ownership remains with the supervisor.
Failures remain in CloseReport until explicitly reconciled/acknowledged. Dropping
the final store requests all cleanup; keep the owner, supervisor and executor
alive until drained. No new ownership framework or delete operation is introduced.
See `packages/service/tests/handle_cleanup.rs` for the runnable producer example.

Generic protocols with asynchronous cleanup must implement close_wait and retain
cleanup independently of its callers. The default is appropriate only for fully
synchronous close. A handle's own asynchronous operations must retain whatever
resources their accepted work needs; merely cloning a handle does not imply join.

## Persistence acknowledgement and recovery

File backings expose `Durability::Buffered` (default) and `Durability::Synced`.
Buffered acknowledges completed OS writes, not power-loss durability. Synced
requires an existing parent directory and support for file/directory sync.
Whole-file save writes a unique sibling temporary file, synchronizes it, atomically
replaces the target, then synchronizes the parent directory. JSONL append writes
a newline-terminated record, synchronizes the file and then its parent, including
when the file was newly created. Buffered mode can create parent directories;
Synced mode deliberately leaves directory creation/durability to the caller.

Acknowledgement depends on the filesystem honoring these operations. These are
single-writer helpers, not interprocess locking or transaction coordinators.
Tests inject failures before write, file sync, rename and directory sync and
check reopen state. A subprocess test reopens acknowledged files after exit
without destructors; these tests do not simulate power loss or every filesystem.

BackedStore stages each candidate in memory, saves it, then publishes it. LogStore
likewise publishes only after backing acknowledgement. A save/append error fences
subsequent writes; reads retain the last acknowledged memory state. `recover()`
explicitly adopts readable backing state and clears the fence only on success.
The recovered state can contain an unacknowledged write. Retrying a non-idempotent
operation without reconciliation can duplicate effects; application operation IDs
or transactions remain application responsibilities. Errors after replacement or
sync failure do not imply rollback.

JSONL open and append reject incomplete trailing records, including valid JSON
without its terminating newline. Invalid UTF-8/JSON also fails loading. No bytes
are silently discarded. An operator must decide whether to truncate an incomplete
suffix or recover it, then reopen. This policy protects acknowledged entries from
accidental tail concatenation while preserving evidence of ambiguous writes.

## Streaming HTTP and event framing

Enable `structfs-http/streaming` (or `structfs/http-streaming`).
AsyncHttpExecutor exposes status and headers before consuming the body;
ByteStream pulls fallible byte chunks. AsyncReqwestExecutor forwards method,
query, headers and optional JSON body and owns no extra per-response producer
task. Dropping the response body abandons transport consumption. Explicit
`read_limited` bounds buffered responses such as error bodies. Configure timeouts
on the executor; consumers that stop early must drop the body.

SseFramer is independently usable without native HTTP. It handles split UTF-8,
LF/CRLF/CR, one optional space after a field colon, multiline data, comments and
field metadata. Its size bound covers event content and normalized line endings;
CRLF counts as one line ending. EOF emits pending data even without a final blank
line, an explicit framing choice. It emits transport frames only: provider event
names and completion sentinels have no special meaning. The caller owns reconnect
state (including event IDs); the framer does not implement EventSource reconnect.

Errors are terminal and output preserves already completed events before a later
error in the same chunk. The frame limit bounds parser retention, not the number
of completed frames in a caller-provided input chunk. Consumers should process
chunk outputs promptly and budget their own queues.

## Names-only discovery

Mount `ChildNames` beside the data store. Its read addresses are
`offset/limit/target...`; replies are `{names, next}`, where null next means end.
Missing targets return absence; leaves and empty containers return empty pages;
arrays enumerate indices. Limits bound page item count and total UTF-8 name bytes.
The projection rejects invalid/nonadvancing cursors and oversized replies.

It forwards Reader::read_children_page. The default uses read_children, preserving
a raw-record provider's names override, but materializes all names before paging.
MemoryStore pages directly; large external providers must override paging to
bound their own temporary memory. Offset traversal may still take linear time.
Offsets are not snapshot tokens: use an immutable/versioned provider for stable
replay through concurrent mutation. Encoding overhead is a separate response
budget. Put the projection at the provider boundary before async/service adapters;
its ordinary reads then traverse existing mounts, permissions and cancellation.
Core reference/box and read combinators forward paging overrides.

## Diagnostics, caching and masking

Codec errors carry optional `{kind, operation, format, message}` detail. Serving
error envelopes retain it under `error.codec`. The core-Wasm error payload for
codec failures is a UTF-8 JSON envelope with `structfs_error:1`, printable message,
and codec detail; other errors remain plain diagnostic text. Updated Rust SDK
HostError decodes the envelope and exposes optional codec fields. Older guests
still receive printable diagnostics, including the message inside the envelope.
Codec resource limits map to status -8 and serving `resource_limit`; other codec
failures retain status -9. Unknown detail can be ignored. This does not change
absence, successful values, or provider-specific profile fault envelopes.

LazyRecord serializes fallible initialization. Successful values are cached;
errors are retryable and not cached. A later caller may try a different codec;
the first successful decoder determines the cached value. Decoder panics publish
nothing. A codec must not recursively request the same record while decoding it.

Masked only redacts directly matched requests, not matching descendants in an
ancestor record, and child names remain visible. Its rustdoc now prominently
states that it is not a subtree-security or snapshot-sanitization boundary.
Capability scoping must prevent unauthorized ancestor reads.

## Typed retained streams

`OwnedTail::push_batch` admits an entire batch atomically or rejects it without
changing items/cursors. `read_bounded` limits both item count and payload bytes
before cloning; an oversized first item errors instead of returning a stalled
page. Terminal streams remain terminal. Consumer encoding/copies have separate
budgets. `packages/service/examples/typed_tail.rs` shows typed events, full-tail
rejection, minimum-cursor acknowledgement across two readers, final status and a
bounded serialization writer. Real transports acknowledge according to their
own delivery guarantee; removing a disconnected reader is an explicit policy.
