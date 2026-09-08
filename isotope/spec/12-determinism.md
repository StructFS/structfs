# Determinism and Replay

This document specifies when a Block's execution is reproducible, and the
vocabulary that makes reproducibility a property a runtime can offer rather
than an accident a Block happens to have. It defines **determinism classes**
for the mounts in a Block's namespace, **transcripts** — the recording of a
run's boundary answers and replay from one — **virtual providers** that
make a live run deterministic with no transcript at all, and the
obligations a runtime must meet for the replay claim to hold. Transcription
and determinism are orthogonal features; the chapter treats them together
because they share one vocabulary and check each other.

Nothing here adds an operation, a path, or an import. Determinism is a
consequence of the architecture the other chapters already specify: a Block
is single-threaded (`01-blocks.md`), everything it learns from outside
arrives as the answer to a store operation (`04-system-paths.md`,
`06-protocol.md`), and the outside's questions reach it only through the
Server Protocol (`07-server-protocol.md`). This chapter names what follows
from that.

## The Determinism Thesis

A Block's next step is a function of three things: its code, its memory, and
the answer to the store operation it most recently issued. Its code is fixed
at load time and its memory is a function of its history, so everything that
can vary between two runs enters through the boundary — as the answer to a
read, or the acknowledgement of a write.

Therefore:

> **The sequence of boundary answers is the entire nondeterminism of a
> run.** Record that sequence and the run is recorded. Serve the same
> sequence again and the run happens again, step for step.

This is the thesis the rest of the chapter operationalizes. It holds only
when the runtime closes the side channels (see *Runtime Obligations*), and
only for the operations that are actually inputs to the run — which is what
the classes distinguish.

## Two Orthogonal Features, Neither Mandatory

This chapter describes **two independent, optional runtime features**,
and a runtime is conformant with neither, either, or both:

- **Transcription** — recording the boundary answers of a run, and
  replaying a run from a recording. A transcript is a *record*, not a
  promise: any run can be transcribed, a nondeterministic one included,
  purely as an account of what the world said.
- **Determinism** — serving a Block's `input` mounts from virtual
  providers (seeded entropy, a virtual clock, simulated peers) so that
  two runs with the same configuration are the same run, with no
  transcript involved.

They mix and match. A live nondeterministic run can be recorded for
audit. A seeded deterministic run needs no transcript to be
reproducible — rerunning it *is* reproducing it. And the combination is
an instrument: two seeded runs must leave identical transcripts, and
replay of any transcript **tests** determinism — a divergence error is
the discovery, made loudly and at the exact operation, that the run was
not a function of its recorded inputs.

A runtime that implements neither feature may run every Block live,
against the real clock and the real network, with nothing recorded and
none of the *Runtime Obligations* below in force. Those obligations bind
only where a faithful replay is intended.

What this chapter standardizes is the vocabulary and the semantics, so
that when a runtime does offer these features, transcripts mean the same
thing everywhere:

- **Classes are declarations, not behavior.** A mount's determinism class
  changes nothing about how operations on it behave in a live run. An
  Assembly that declares classes runs identically on a runtime that
  ignores them.
- **Blocks are indifferent.** A Block must not require a deterministic
  host, and cannot detect one: a recorded run, a replayed run, a seeded
  run, and a plain live run present the same boundary. Code written for
  this spec needs no changes to become replayable — that is the point.
- **The features are per-run, and may be per-Block.** A runtime may
  record one Block of an Assembly and run the rest live, seed one
  Block's providers and not another's, or offer none of it today and add
  it later without an interface change.

## Determinism Classes

Every mount in a Block's namespace belongs to exactly one class. The class
says what the mount means to a recording and to a replay.

### `input`

State flows from the world into the Block. The answers to operations on an
`input` mount are inputs to the run: they are recorded, and on replay they
are served from the recording instead of from the world.

Examples: `/iso/time`, `/iso/random`, `/iso/config`,
`/iso/shutdown/requested`, an inbound data stream, the Block's mailbox.

### `effect`

State flows from the Block into the world. The *payload* of a write to an
`effect` mount is an output of the run — a recording may keep it for
inspection, but replay does not need it. The *acknowledgement* of that
write (the result path, or the refusal) is an input like any other: the
Block branches on it, so it is recorded and replayed.

On replay, **the effect is not re-executed**. Replaying a run is
reproducing a computation, not repeating its consequences: the same bytes
must not go to a real socket a second time, the same message must not be
logged into a live system twice. The transcript answers the write;
nothing leaves.

Examples: `/iso/console`, `/iso/log`, `/iso/shutdown/complete`, an
outbound data stream.

### `observation`

The world looking at the Block, without touching it. Operations in this
class are **outside the recording entirely** and are served live even
under replay.

The Server Protocol is the canonical member: serving a Request reads the
Block's state and writes a Response, and must change nothing the Block's
own computation can observe. That property is what earns the class — a
debugger can interrogate a replayed run at any instant, ask questions the
original run was never asked, and the replay cannot diverge, because the
answers to observations were never inputs.

`observation` is a proof obligation, not a label of convenience. A mount
may be declared `observation` only if serving it is invisible to the
Block's computation. A runtime that lets an observation perturb the Block
— refill a cache the Block can time, advance a counter the Block can read
— has misclassified it, and the failure will surface as replay
divergence.

### Class Defaults

The system paths carry these classes unless an Assembly overrides them:

| Mount | Class |
|---|---|
| `/iso/time` | `input` |
| `/iso/random` | `input` |
| `/iso/config` | `input` |
| `/iso/shutdown/requested` | `input` |
| `/iso/server/requests`, `…/requests/pending` | `input` * |
| `/iso/console`, `/iso/stdio` | `effect` |
| `/iso/log` | `effect` |
| `/iso/shutdown/complete` | `effect` |
| `/iso/self/interface` | `effect` |
| `/iso/server/responses` | `observation` |

\* A served Request is an input — the Block's computation consumes it and
acts on it — but see *The Server Protocol Split* below for the
debugger-driven exception.

Wired services (`02-assemblies.md`) default to `input` for the reads a
Block issues against them and `effect` for the writes, per the rules
above. An Assembly may narrow this (see *Declaration*).

### The Server Protocol Split

The Server Protocol appears on both sides of the line, and the split is
the subtle part of this chapter.

Requests that arrive because a *peer* needs something from the Block are
work: the Block computes on them, its state changes, and its subsequent
questions to the boundary depend on them. Those Requests are `input` —
recorded, replayed.

Requests that arrive because a *debugger* is looking at the Block are
observations: the handler reads state and writes a Response and the
Block's computation cannot tell it happened. Those must stay off the
transcript, or a replay could not be inspected without diverging.

A runtime that serves both kinds through one queue must either classify
per-request (a Request marked observational by the runtime that enqueued
it) or dedicate an observation-class request path distinct from the
working mailbox. Which mechanism is chosen is a runtime concern; that
the two kinds have different classes is not.

## Recording

A recording — a **transcript** — is the ordered sequence of boundary answers
given to one Block, for operations on `input` and `effect` mounts.

Recording requires nothing of the run. It works on a nondeterministic
run exactly as well as on a seeded one — the transcript is then an audit
record rather than a replay source, and replaying it is how that
difference is found out.

Each entry holds:

- the **operation** (`read` or `write`) and the **path** the Block asked,
- the complete **answer**: the value, the result path, *the fact that the
  path held nothing*, or *the refusal and its error*.

Three rules make a transcript correct:

1. **A refusal is an answer.** A read of a path nothing is mounted at
   fails, and that failure is something the Block branches on — an
   unmounted `/iso/config/trace` is how a Block learns it is not being
   traced. A transcript that keeps only successful answers shifts every later
   entry by one, and the replay then hands the Block its entropy where it
   asked for a flag. Record everything, including `not_found`, including
   errors.

2. **Write acknowledgements are inputs.** The result path of a write, or
   its refusal, flows into the Block's computation exactly as a read
   answer does. A transcript that records reads only cannot replay a Block that
   ever branched on whether a write succeeded.

3. **The path is recorded beside the answer.** It is not redundant: it is
   the divergence check (see *Replay*).

Because a Block is single-threaded, its boundary operations are totally
ordered and the transcript is a simple sequence. No timestamps, no
synchronization, no merge.

A transcript should be presented *as a store* — an append log read
through the same cursor conventions as any other log — so it composes
with existing tooling: mounted, paged, diffed, and cascaded over, with
nothing built specially for it. How the store persists it is the
store's business.

### The Interop Format

A transcript that must travel between runtimes uses this format —
runtimes that speak it can replay each other's recordings, which is
the property that makes a Block's run portable evidence rather than
one host's private state. One JSON object per entry, in order (JSON
Lines when written as a file):

```
{"op": "read" | "write",
 "path": "<the path asked, as a string>",
 "answer": <answer>,
 "wrote": "<payload digest, writes only, optional>"}
```

The answer is one of four shapes, mirroring the four things a boundary
can say:

| The world said | `answer` |
|---|---|
| a value | `{"found": {"parsed": <value>}}` |
| nothing at that path | `"absent"` |
| a write acknowledgement | `{"wrote": "<result path>"}` |
| a refusal | `{"failed": {"kind": "<kind>", "message": "...", "path"?: "..."}}` |

Error kinds are the closed set `not_found`, `no_route`,
`permission_denied`, `conflict`, `overloaded`, `deadline_exceeded`,
`resource_limit`, `cancelled`, and `other` — replayed code must branch
on the same typed error the recorded run saw. Integer values are
written at full precision; a replaying host whose numbers are narrower
than the recording's must preserve the exact rendering (nanosecond
timestamps exceed 2^53).

The `wrote` digest is fnv1a-64, lowercase hex, over the UTF-8 of the
answer envelope's value form (`{"parsed":<value>}` with the value's
canonical rendering). It is optional per entry: a host records it only
when it can render the payload unambiguously (strings, booleans, null,
exact integers — canonical JSON of composites is a host's own affair
until a canonicalization is pinned), and replay compares digests only
when both sides have one. A missing digest weakens divergence
detection for that entry; it never causes one.

## Replay

To replay, the runtime mounts the transcript where the world was:

- An operation on an `input` or `effect` mount is answered by the next
  transcript entry. No store is consulted; no effect is performed. **The transcript
  is the host.**
- An operation on an `observation` mount is served live, exactly as in a
  recorded run.

Before answering, the runtime compares the operation and path the Block
asked against the operation and path the transcript recorded:

- **A match** is answered.
- **A mismatch is a divergence**, and the replay fails loudly at that
  entry. A replayed Block that asks a different question than the
  recorded one did is a Block that was not a function of its inputs —
  the determinism thesis is false for it, and the divergence point is
  where the hidden nondeterminism is found. Failing loudly at the first
  divergence is the feature; a replay that quietly goes somewhere else
  is worse than none.
- **A transcript that runs out** while the Block still asks is the same
  failure with the same reporting.

A runtime may additionally record a **digest of each write's payload**
and check it on replay. The payload itself is an output and stays off
the transcript, but a run that writes *different data* to the recorded
path has diverged just as surely as one that asks a different question —
without the digest, the replay would acknowledge data the recorded world
never saw.

A replay needs no capabilities. A Block replayed from a transcript can be run
with nothing mounted but the transcript and its observation paths — no network,
no clock, no entropy — which is itself a statement of what a transcript is: the
world, as one run experienced it.

## Checkpoints

The Lifecycle chapter (`05-lifecycle.md`) leaves checkpointing open. The
transcript closes most of it:

> A **checkpoint** is a memory image plus a transcript cursor, taken at a
> quiescent point.

A **quiescent point** is any moment at which the Block is between boundary
operations with no operation in flight — for a run-loop Block, between
turns. At a quiescent point the Block's entire state is its memory (and
the store-side state of its mounts, for stores whose class requires it);
the transcript cursor says how much of the world it has consumed.

Restoring a checkpoint and replaying the transcript from its cursor reproduces
every subsequent state of the recorded run. Seeking *backward* is
restoring the nearest earlier checkpoint and replaying forward — which is
how a time-travel debugger is a corollary of this chapter rather than a
feature of its own.

What a checkpoint must capture beyond memory — and what it may safely
drop, such as caches whose contents the Block cannot distinguish from
their absence — is a contract between the runtime and its stores, out of
scope here.

## Runtime Obligations

The thesis holds only if the boundary is the *only* way the world reaches
the Block. These obligations attach to **faithful replay**, not to
transcription: for a replay to reproduce a run, both the recorded run and
the replaying run must meet them. Recording alone requires none of this,
and a transcript of a run that met none of it is still a true record of
what that run was answered. A runtime intending replay must ensure:

1. **No side-channel imports.** The Block imports the store operations and
   nothing that varies: no clock import, no random import, no host
   information reachable outside the store.
2. **Deterministic execution.** The engine must execute the same code on
   the same memory to the same result: canonicalized NaNs (or a policy
   with the same effect), deterministic memory growth behavior,
   deterministic trap points.
3. **Deterministic scheduling.** Within a Block, single-threadedness
   provides this. Across an Assembly, the runtime must not let scheduling
   leak into any Block's answers except through recorded entries — which
   per-Block transcripts give by construction (see below).
4. **Observation invisibility.** Serving an `observation` operation
   changes nothing the Block's computation can observe. This is the
   obligation that licenses the class.
5. **Metering invisibility, or metering on the transcript.** If the Block can
   observe its own fuel or instruction count, that observable is an input
   and must be recorded like one.

## Assemblies

Transcripts are **per-Block**. An Assembly's recording is one transcript per member
Block, not a global order: each Block's boundary is totally ordered by
that Block's own execution, and no cross-Block order needs to be — or
correctly can be — imposed.

This is what makes replay decompose. One Block of an Assembly can be
replayed against its transcript while its peers run live, because everything a
peer ever did to it arrived as entries on its own transcript. Replaying a
Block against a recording of a service it was wired to — with the real
service absent — is the same mechanism as replaying it against a
recorded clock.

A wiring point is classified from each end: the caller's operations on
the wire are `input`/`effect` on the caller's transcript; the Requests they
become are `input` on the callee's. Recording both sides is valid and
redundant; recording either side alone is sufficient for that side's
replay.

### Declaration

Classes are declared where namespaces are declared: in the Assembly. The
system-path defaults above apply unless overridden.

```yaml
assembly: user-service

blocks:
  api: ./api-block.wasm
  cache: ./cache-block.wasm

public: api

wiring:
  api:/services/cache -> cache

determinism:
  api:
    /services/cache: input     # the default; shown for illustration
    /iso/metrics: effect       # emission only; replay must not re-emit

record:
  api: ./api.transcript              # record this Block's boundary
```

`record` and its counterpart `replay` name which Blocks' boundaries are
transcribed and where. Their presence changes no Block's behavior — a Block
cannot tell whether it is being recorded, and must not be able to tell it
is being replayed except by the absence of capabilities it never checks
for by another channel.

## The Session Log

Per-block transcripts deliberately record no cross-block order, and
replay must never depend on one. But the interleaving is real, and it
is what a timeline debugger, a post-incident investigation, and the
question "what actually happened across this Assembly?" all want. The
**session log** is that view: an optional, runtime-wide,
arrival-order witness of every boundary operation across every Block.

One entry per operation, in the order the runtime's witness observed
them:

```
{"seq": <dense from 0>,
 "block": "<the Block's transcript key>",
 "op": "read" | "write",
 "path": "...",
 "outcome": "found" | "absent" | "wrote" | "failed:<kind>",
 "entry": <index into the Block's transcript, when one is kept>}
```

Properties that make it sound:

- **Observation-class, by construction.** The log is written after
  each operation completes and answers nothing. Replay never reads
  it; a recording is complete without it; appending must never fail
  the operation observed. It works in every mode — live (a flight
  recorder with no transcripts at all), recording (`entry` links each
  witnessed operation to its full answer in the Block's transcript),
  and replay (the re-run writes its own timeline, comparable to the
  original's).
- **A witness, not a contract.** `seq` is the order operations
  reached the log — an honest record of one observed interleaving,
  never an ordering the Blocks agreed to or that a replay must
  reproduce. Per-block transcripts remain the only replay authority.
  In a distributed Assembly each runtime keeps its own session log;
  no global clock is implied or required.
- **The summary is legible alone.** `outcome` is enough to read a
  timeline without joining; the `entry` link recovers full answers
  when depth is needed.

## Open Questions

1. **Shared stateful stores.** Two Blocks wired to one stateful store
   interleave their operations on it. Each Block's transcript replays that
   Block correctly, but re-running the *store* from both transcripts needs a
   merge order the transcripts do not carry. The session log witnesses
   that interleaving from the runtime's side; is the store's own log
   the authoritative third transcript, with the session log as its
   cross-check?

2. **Effects the Block reads back.** A Block that writes an effect and
   later reads its consequence through an input mount (writes a file
   via a wired store, reads it back) is consistent on transcript — both
   crossings are recorded — but a *live* re-execution against a
   partially-replayed world could tear them. Does partial replay need a
   consistency declaration between mounts?

3. **Large payloads.** A Block that streams its own image or dataset
   through the boundary produces a transcript dominated by bulk data. Should
   the spec bless content-addressed payloads (entries holding hashes
   into a blob store) as a portable transcript optimization?

4. **Divergence localization.** A path mismatch locates divergence at
   the boundary — and the session log places it on the Assembly's
   timeline — but the cause is earlier, inside the Block. Should
   transcripts carry optional progress markers (instruction counts, logical
   clocks) to bisect against?

5. **Declaring simulation.** Virtual providers are the determinism
   feature (see *Two Orthogonal Features*), and Blocks must not be able
   to detect them casually. But should the spec name a standard for a
   host to *voluntarily* declare that a namespace is simulated, so a
   Block that wants to refuse to run against simulation — a
   certificate-issuing service, say — can ask?

6. **Standard provider semantics.** Seeded entropy has one obvious
   meaning; a virtual clock has several (fixed epoch advancing per read,
   scaled real time, event-driven logical time). Should the spec pin one
   per `/iso/time` path, so a seed means the same run on every runtime?
