# Isotope Spec: Open Topics

This document tracks outstanding design questions and issues that need resolution
before the spec is complete.

## Priority 1: Blocking Issues

These prevent correct implementation or understanding of the spec.

### 1.1 Concurrency Model Clarity

**Status:** Needs spec text

**Problem:** The spec says Blocks are single-threaded and reads can block, but
doesn't explain the cooperative concurrency model. Block authors will misunderstand
what "blocking" means.

**Questions:**
- When a Block calls `read("/services/db/query")`, can it serve other requests
  while waiting? (Answer: No)
- What are the deadlock implications?
- How does the runtime (async, like Wasmtime+Tokio) relate to the Block's
  synchronous view?

**Resolution:** Add a section to `01-blocks.md` or `07-server-protocol.md` that
explicitly states:
- Blocks are cooperative, not preemptive
- A "blocked" Block yields to the runtime but cannot serve requests while waiting
- Deadlocks are possible; design wiring graphs accordingly
- The handle pattern for async operations

**Relevant files:** `01-blocks.md`, `07-server-protocol.md`

---

### 1.2 Error Model for Wiring Failures

**Status:** Needs design

**Problem:** If Block A writes to `/services/cache/key` and the cache Block is
in `Failed` state, what error does A see?

**Questions:**
- Is it `unavailable` (retryable) or something else?
- Can A distinguish "path doesn't exist" from "target Block is dead"?
- How do `failure: isolate` vs `failure: restart` affect the error?

**Proposed approach:**
- `isolate` → returns `unavailable` with `retryable: true` (Block might recover)
- `fail-fast` → Assembly fails, so A wouldn't be running to see the error
- `restart` → returns `unavailable` with `retryable: true` while restarting

**Relevant files:** `02-assemblies.md`, `06-protocol.md`

---

### 1.3 Secrets Management

**Status:** Needs design

**Problem:** Secrets can't go in Assembly definitions (visible in
`/iso/assemblies/*/definition`). No guidance on how to handle them.

**Requirements:**
- Secrets not stored in definition
- Scoped access (Block A can read secret X, Block B cannot)
- Rotation without redeployment
- Audit logging (who accessed what)

**Options:**
1. **Blessed path pattern:** `/iso/secrets/{name}` provided by runtime
2. **External store wiring:** Wire a secrets Block like any other service
3. **Config reference:** `config.db_password: { secret: "db-password" }` syntax

**Leaning toward:** Option 2 (external store wiring) with a standard interface.
Keeps runtime simple, secrets management is a Block like anything else.

**Relevant files:** `04-system-paths.md`, `08-assembly-management.md`

---

### 1.4 Startup Ordering and Readiness

**Status:** Needs design

**Problem:** Blocks start lazily on first access. No way to know if a Block is
"ready" to serve (initialized, connections established, etc.).

**Scenario:**
```
A starts, writes to /services/db/query
→ db Block starts (lazy)
→ db Block is in "Starting" state, not ready
→ A's query fails or times out
```

**Options:**
1. **Readiness protocol:** Block writes to `/iso/self/ready` when ready.
   Operations block until target is ready.
2. **Health check pattern:** Standard `/health` or `/ready` path that Blocks
   implement. Callers can poll.
3. **Accept-and-queue:** Requests to starting Blocks queue until ready.
4. **Caller handles it:** Document that Blocks must handle "not ready" errors
   gracefully.

**Leaning toward:** Option 3 (accept-and-queue) as default, with option 1
available for explicit control.

**Relevant files:** `01-blocks.md`, `05-lifecycle.md`

---

## Priority 2: Implementation Blockers

These are needed before a runtime can be built.

### 2.1 Artifact Resolution Protocol

**Status:** Needs design

**Problem:** Block references like `./api-block.wasm` or `registry/foo/bar` need
to resolve to actual Wasm bytes. No protocol defined.

**Questions:**
- What's the base path for relative references?
- What's the registry protocol?
- How are artifacts fetched, cached, verified?

**Proposed approach:**
Define resolution as runtime-specific, but specify the contract:
- Given reference + hash → runtime provides bytes matching hash
- If hash doesn't match, instantiation fails
- Caching/fetching is runtime concern

For registries, suggest (but don't require) an OCI-compatible protocol.

**Relevant files:** `02-assemblies.md`, new `09-artifact-resolution.md`?

---

### 2.2 Spec Versioning

**Status:** Needs design

**Problem:** No way for an Assembly to declare which Isotope spec version it
targets. Runtimes can't know if they support a given definition.

**Proposed:**
```yaml
isotope: "1.0"
assembly: user-service
version: 2024.01.15
```

The `isotope` field declares spec compatibility. Runtimes reject Assemblies
targeting unsupported spec versions.

**Relevant files:** `02-assemblies.md`, `00-overview.md`

---

## Priority 3: Important Clarifications

These don't block implementation but cause confusion.

### 3.1 Config Injection Semantics

**Status:** Needs clarification

**Problem:** The spec says config appears at `/config/` but doesn't specify:
- Is `/config/` a wired store or special namespace?
- What happens if a Block writes to `/config/`?
- When is config injected relative to Block startup?

**Proposed:**
- `/config/` is a read-only store provided by the runtime
- Writes to `/config/` return `not_writable` error
- Config is available before the Block's first read from `/iso/server/requests`

**Relevant files:** `02-assemblies.md`, `03-namespaces.md`

---

### 3.2 Observability Paths

**Status:** Needs decision

**Problem:** Metrics, traces, and structured logs are mentioned as "extensions"
but not standardized. Without standards, tooling can't aggregate across Blocks.

**Options:**
1. **Standardize now:** Define `/iso/metrics/`, `/iso/trace/`, `/iso/logs/`
2. **Defer:** Mark as "v2" and let implementations experiment
3. **Interface only:** Define the shape of metrics/traces, not the paths

**Leaning toward:** Option 1 for basic metrics (counters, gauges), Option 2 for
tracing (more complex, less consensus).

**Relevant files:** `04-system-paths.md`

---

### 3.3 Canonical Definition Format

**Status:** Needs decision

**Problem:** Examples use YAML, management API shows JSON. Which is canonical?

**Options:**
1. **YAML canonical:** Human-friendly, comments allowed
2. **JSON canonical:** StructFS-native, unambiguous
3. **Either:** Define semantic equivalence rules

**Leaning toward:** Option 3. Definitions are StructFS Values. YAML and JSON
are serializations. Semantic equivalence = same Value.

**Relevant files:** `02-assemblies.md`, `08-assembly-management.md`

---

## Priority 4: Future Work

Not blocking for v1, but should be tracked.

### 4.1 Testing Infrastructure

**Notes:**
- How to inject non-Wasm mock Blocks for in-process testing?
- How to assert on Block interactions?
- How to simulate failure modes?

Probably tooling, not spec. But guidance in examples would help.

---

### 4.2 Hot Reload / Live Update

**Notes:**
- Can a Block be replaced without restarting the Assembly?
- What happens to in-flight requests during replacement?
- State migration between old and new Block instances?

Complex. Defer to v2.

---

### 4.3 Distributed Assemblies

**Notes:**
- Assemblies spanning multiple machines
- Network partitions, partial failures
- Consistency across distributed Blocks

Out of scope for v1. Single-runtime Assemblies only.

---

### 4.4 Resource Limits and Quotas

**Notes:**
- Memory limits per Block
- CPU quotas
- Request rate limiting
- How does a Block discover its limits? `/iso/limits/*`?

Defer to v2, but design `/iso/limits/` path structure.

---

## Discussion Log

### 2024-01-19: Initial Review (Royb)

Key feedback:
- Core model is sound (stores all the way down, Assemblies as Blocks)
- Blocking/async model needs clarification—API looks sync but runtime is async
- Single-threaded Blocks are a scaling bottleneck → solved via composition
- Data model under-specified for real protocols (protobuf mapping)
- No persistence or distribution story (acceptable for v1)
- "Empowering app engineers" framing is optimistic—complexity relocated, not removed

Decisions made:
- Scaling via composition (router Blocks), not runtime features
- Assemblies are immutable values with structural sharing
- Management API is StructFS (no separate API)
- Content-addressed Blocks for deduplication

---

## Next Steps

1. Write concurrency model clarification (1.1)
2. Design error model for wiring failures (1.2)
3. Design secrets pattern (1.3)
4. Design readiness protocol (1.4)
5. Add spec version field to Assembly schema (2.2)
