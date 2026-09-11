# Embedding and capability service profiles

This release adds an embedding contract alongside the core Block ABI. It does
not rename existing `iso/` paths or change the core-wasm import module.

## Three lifetimes

A prepared artifact is reusable code, identified and admitted for compilation by
the embedding host. An instance owns its namespace, execution state, memory and
provider resources. A request owns its outstanding operations, deadline and
cancellation; multiple requests may address one persistent instance.

Request cancellation removes pending correlations and refunds admission. It does
not terminate a shared instance, undo committed effects or interrupt unrelated
requests. A cooperative server cancels request-owned work when its response
identity is cancelled. Native adapters obtain a token through
`Namespace::request_cancellation(respond_to)`; core guests may read
`iso/server/cancelled/{token}`. The result is true for expired/unknown identities.
Servers must bound their internal work queues and observe cancellation at safe
points. CPU used by a shared interpreter is not automatically attributable to an
individual request.

Instance shutdown stops admissions, requests graceful exit, escalates after one
grace deadline and joins execution tasks. `ShutdownReport.remaining` identifies
work not joined; hosts must retain its reservations. Blocking native code can
ignore cancellation and cannot be forcibly reclaimed. Dropping an ordinary
assembly reference is not shutdown.

## Artifact adapters

Hosts prepare artifacts asynchronously using their adapter's own API and bounded
compilation resources, then register an `Arc<dyn WasmBlockDriver>` with
`Runtime::register_artifact`. File-based `ArtifactLoader` remains available.
Each `execute(DriverContext)` receives a fresh namespace, format, cancellation,
instance execution scope, shared call budget, metering policy and instance meter.
The core Wasm driver uses this same entry point. Adapters must join child tasks
and release execution resources before returning, and enforce or explicitly
reject unsupported requested limits. Rust adapters are trusted host code.

The legacy synchronous `run`/`run_async` bridge remains for existing adapters.
It receives the original fuel/epoch policy but reports no measurements unless
updated; missing measurements are represented as absent, never zero.

## Reserved runtime paths

`iso/` remains runtime-owned. Assemblies cannot shadow it.

| Path | Contract |
| --- | --- |
| `iso/self/*` | Instance identity and interface |
| `iso/shutdown/*` | Instance lifecycle |
| `iso/server/requests` | Existing event mailbox |
| `iso/server/requests/pending` | Existing nonblocking event batch |
| `iso/server/responses/{token}` | Resolve one served request |
| `iso/server/cancelled/{token}` | Read whether that request is finished/cancelled |
| `iso/execution/budget` | Read-only policy and measured usage |
| `iso/capabilities` | Read-only array of granted mount prefixes |

Capability discovery describes the namespace, not everything each provider can
do. It reveals no host paths or secrets and grants no new authority. A mount's
own `meta`/documentation describes its operations; discovery is not proof that
any particular operation will succeed. Existing time, entropy, stdio, logging,
environment and timer paths remain the compatibility profile. `proc` remains
explicitly granted, despite being under `iso/`.

Do not add runtime-specific `iso/net`, `iso/config` or filesystem branches.
Application configuration and external services are assembly grants. Suggested
mount names are `config`, `services/clock`, `services/entropy`,
`services/console`, `services/network`, `services/storage`, and
`services/telemetry`; these names are conventions, not reserved prefixes.

## Capability service schemas

Providers declare which profile they implement. Missing capabilities fail with
PermissionDenied at namespace routing; a granted provider uses typed errors for
unsupported operations. No provider silently substitutes invented clock or
entropy values. Deterministic providers are explicit grants.

| Profile | Relative operations and values |
| --- | --- |
| Clock | read `realtime_ns`: Integer Unix nanoseconds; read `monotonic_ns`: Integer nanoseconds relative to a provider-defined origin; read `after/{ms}`: cancellable wait, Null on completion |
| Entropy | read `bytes/{n}`: exactly n Bytes; provider bounds n and total admission |
| Binary stream | read `rx/{max}`: Bytes, empty only at EOF; write `tx`: Bytes; readiness and half-close as below |
| Console | `stdin`, `stdout`, `stderr` submounts implement the binary-stream profile; providers restrict directions |
| Telemetry | write `log/{level}`: `{msg: String, fields: Map}`; bounded delivery must reject overload explicitly |

Network providers expose granted connection handles implementing the stream
profile. Listener creation, port publication and outbound routing require an
explicit provider contract and host authority; this release does not prescribe a
universal network manager. Likewise, it does not prescribe database transactions
or durable filesystem semantics. Their grants retain their own schemas.

`structfs-handles::DuplexStream` implements the consuming stream profile:
`ready/read`, `ready/write` and `ready/both` park for advisory readiness; Null to
`shutdown` half-closes transmit; Null to the root releases both directions. Every
buffer has a capacity, oversized chunks fail and full buffers park writers.
Dropping pending futures transfers no bytes. See the duplex-stream pattern in
StructFS documentation for ownership and teardown rules.

## Budgets and observations

`iso/execution/budget` is an instance view with `calls` (child-to-root budget
snapshots), `events`, `replies`, `execution`, `deadline_remaining_ms` and `units`. A budget
snapshot contains policy revision, limits and usage; call snapshots also contain
cumulative admissions/rejections and peaks. Each snapshot is atomic; a hierarchy
is sampled separately. Guests cannot write these paths to raise limits.

Call bytes are logical payload weight. Session memory is reserved capacity.
Execution memory is observed Wasm linear memory, not process RSS or compiler
memory. Fuel is Wasmtime fuel, not CPU time or emulated x86 instructions. Driver
counters name their units. Unknown measurements are absent. Core-Wasm samples at
imports, epoch yields and termination; after joined termination current memory
is zero and peak memory/fuel remain inspectable on the retained cell meter.
Policy updates retain charges; existing work is grandfathered and subsequent
admissions must fit. Fuel/deadline/memory execution-policy mutation is not implied
by admission-policy mutation.

Signals and registered timers share a bounded per-instance event budget. Timers
reserve capacity before sleeping and keep the reservation until expiry is consumed
or the timer is cancelled. Caller traffic is governed by call admission. Replies retain a separate byte
charge until consumed; an oversized reply returns a typed overload error. Shutdown
never requires an extra mailbox slot. Host-provided log sinks, provider queues and
interpreter-internal queues remain separately bounded by their owners.

## Optional resumable execution

An adapter may advertise `DriverCapabilities { resumable, checkpoints }` and
provide a host-only per-instance `DriverControl` store. This is an extension
point, not a claim that an ordinary suspended async import is snapshot-safe.
The adapter documents its control schema, safe points, state version and artifact
identity. Checkpoint support requires provider-state capture, validated rebinding
or explicit rejection. Live sockets cannot be reconstructed from guest memory.

Inspection is not automatically excluded from replay. An adapter may expose a
separate host-only inspection surface whose operations cannot change guest-visible
state. Application reads and writes remain in the effect transcript. Debugging
must never make arbitrary application reads replay-exempt. This release does not
provide a universal snapshot/restore or debugger ABI.
