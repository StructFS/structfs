# Server Protocol

The **Server Protocol** defines how Blocks serve StructFS stores. It is the
mechanism by which a Block—which is internally a StructFS client—presents
itself as a StructFS store to the outside world.

## Overview

A Block has two perspectives:

- **Inside**: The Block is a StructFS client. It reads and writes paths.
- **Outside**: The Block is a StructFS store. Others read and write to it.

The Server Protocol bridges these perspectives. The runtime:

1. Receives operations destined for a Block's store
2. Packages them as Requests
3. Delivers them to the Block via `/iso/server/requests`
4. Receives Responses from the Block
5. Delivers Responses back to callers

The Block never "serves" directly—it just reads Requests and writes Responses.
Like a POSIX program reading from stdin.

## Request Structure

A Request is delivered when the Block reads from `/iso/server/requests`:

```json
{
    "op": "read",
    "path": "users/123",
    "data": null,
    "respond_to": "/iso/server/responses/a1b2c3"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `op` | string | `"read"` or `"write"` |
| `path` | string | Path being accessed (relative to Block's store root) |
| `data` | Value | Data being written (null for reads) |
| `respond_to` | string | Path where Block should write its Response |

### Path Relativity

The `path` field is relative to the Block's store root. If a caller writes to:

```
/services/cache/users/123
```

And the wiring is:

```yaml
api:/services/cache -> cache
```

Then the cache Block receives:

```json
{"op": "write", "path": "users/123", ...}
```

The `/services/cache` prefix is stripped. The Block doesn't know where it's
mounted.

## Response Structure

The Block writes a Response to the `respond_to` path:

```json
{
    "result": "ok",
    "value": {"id": 123, "name": "Alice"},
    "path": null
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `result` | string | `"ok"` or `"error"` |
| `value` | Value | Return value (for successful reads) |
| `path` | string | Return path (for successful writes) |
| `error` | object | Error details (when result is "error") |

### Response for Read

```json
{
    "result": "ok",
    "value": {"id": 123, "name": "Alice"}
}
```

The `value` is what the caller's read returns.

### Response for Write

```json
{
    "result": "ok",
    "path": "users/123"
}
```

The `path` is what the caller's write returns. For deferred operations:

```json
{
    "result": "ok",
    "path": "outstanding/42"
}
```

The caller can then read from `outstanding/42` (which becomes another Request).

### Error Response

```json
{
    "result": "error",
    "error": {
        "type": "not_found",
        "message": "User 123 does not exist"
    }
}
```

## Blocking vs Non-Blocking Request Reads

The Server Protocol provides two paths for reading Requests, with different
blocking behavior:

### Blocking: `/iso/server/requests`

```python
request = read("/iso/server/requests")
```

This **blocks** until a Request is available. The Block suspends until:
- A Request arrives, OR
- Shutdown is signaled (returns `null` or shutdown marker)

This is the typical path for simple request-response Blocks.

### Non-Blocking: `/iso/server/requests/pending`

```python
requests = read("/iso/server/requests/pending")
```

This returns **immediately** with an array of all pending Requests. If no
Requests are pending, returns an empty array `[]`.

This enables:
- Batch processing (collect multiple requests, process together)
- Polling patterns (check for work, do other things)
- Custom scheduling (prioritize certain requests)
- Cooperative multitasking within a Block

## Block Run Loop

A typical Block run loop using blocking reads:

```python
while True:
    request = read("/iso/server/requests")

    if is_shutdown_signal(request):
        break

    if request.op == "read":
        value = handle_read(request.path)
        write(request.respond_to, {"result": "ok", "value": value})

    elif request.op == "write":
        result_path = handle_write(request.path, request.data)
        write(request.respond_to, {"result": "ok", "path": result_path})

write("/iso/shutdown/complete", {})
```

The Block:
1. Reads a Request (blocking until one arrives)
2. Dispatches based on operation type
3. Processes and writes Response
4. Repeats

## Batch Processing

A Block can use non-blocking reads for batch processing:

```python
while True:
    # Non-blocking: get all pending requests
    requests = read("/iso/server/requests/pending")

    if not requests:
        # No pending work - could do other things, or block for one
        request = read("/iso/server/requests")  # Block for next
        if is_shutdown_signal(request):
            break
        requests = [request]

    # Process batch
    for request in requests:
        response = process(request)
        write(request.respond_to, response)
```

This pattern allows Blocks to:
- Batch database operations for efficiency
- Prioritize certain request types
- Implement fair scheduling across request types

## Async Operations

The Server Protocol is inherently async. A Block can return immediately with
a handle:

**Request:**
```json
{"op": "write", "path": "jobs", "data": {"task": "process"}, ...}
```

**Immediate Response:**
```json
{"result": "ok", "path": "jobs/outstanding/42"}
```

The caller's write returns `"jobs/outstanding/42"`. Later:

**Request:**
```json
{"op": "read", "path": "jobs/outstanding/42", ...}
```

**Response (when ready):**
```json
{"result": "ok", "value": {"status": "complete", "output": ...}}
```

The Block can block this second read until the job completes. This is the
handle pattern from StructFS.

## Shutdown Signaling

When shutdown is requested, the runtime can either:

1. Return a special "shutdown" Request
2. Unblock `/iso/server/requests` with null
3. Set `/iso/shutdown/requested` to true

The recommended pattern is for Blocks to check `/iso/shutdown/requested`
after processing each request:

```python
while True:
    request = read("/iso/server/requests")

    if request is None or read("/iso/shutdown/requested"):
        break

    process(request)

write("/iso/shutdown/complete", {})
```

## Reentrancy and Cycles

Blocks are single-threaded, but the Server Protocol allows cyclic wiring:

```
Block A writes to Block B
→ B processes, writes back to A
→ A receives B's write as a new Request
```

This CAN work when:
1. A's write to B returns (with a handle or immediate response)
2. A continues (or blocks reading from handle)
3. B's write to A queues as a Request
4. A eventually reads that Request

Example: OAuth callback

```
Auth Block writes to External Service
→ External Service calls back to Auth Block's callback endpoint
→ Auth Block receives callback as Request
→ Auth Block processes callback, completes original flow
```

### Deadlocks Are Possible

**Deadlocks are 100% possible** in Isotope. Because paths served by a Block can
route to other Blocks (via wiring), cyclic dependencies can easily form.

Example deadlock:

```
Block A writes to /services/b/foo (routes to Block B)
→ A blocks waiting for B's response
→ B, while handling, writes to /services/a/bar (routes to Block A)
→ B blocks waiting for A's response
→ A is blocked, cannot read the new Request
→ DEADLOCK
```

### Runtime Responsibilities for Deadlock

The Isotope runtime is responsible for:

1. **Static Analysis**: Warn at Assembly load time when wiring configurations
   could produce deadlocks (cyclic dependencies where all edges are synchronous)

2. **Runtime Detection**: Detect deadlocks at runtime when they occur (similar
   to Go's "all goroutines are asleep" detection)

3. **Deadlock Breaking**: Provide facilities to break deadlocks, such as:
   - Timeout on blocked operations
   - Returning an error to one participant to break the cycle
   - Configurable deadlock policies per Assembly

Blocks that need cyclic communication should use the handle pattern to avoid
synchronous blocking:

```python
# Instead of blocking write:
handle = write("/services/other/request", data)
# Don't immediately read the handle - continue processing requests
# Read the handle later when not blocked on it
```

## Runtime Responsibilities

The runtime implements the Server Protocol by:

1. **Routing**: Mapping external paths to Block stores
2. **Queueing**: Buffering Requests until Block reads them
3. **Delivering**: Making Requests available at `/iso/server/requests`
4. **Correlating**: Matching Responses to waiting callers
5. **Blocking**: Suspending callers until Responses arrive

The Block is unaware of these mechanisms—it just reads Requests and writes
Responses.

## A Note on Examples

The examples below show the raw Server Protocol—manual run loops, explicit
request dispatch, direct response writing. This is analogous to showing TCP
socket code: technically accurate, but not how developers typically work.

In practice, developers use:

- **Language-native SDKs** that generate the run loop and dispatch machinery
- **Protocol wrappers** that translate gRPC, OpenAPI, or other protocols to StructFS
- **Framework conventions** like handler decorators or trait implementations

A real Block might look like:

```rust
#[structfs::store]
impl UserStore {
    #[read("users/{id}")]
    fn get_user(&self, id: u64) -> Result<User, Error> { ... }
}
```

The raw protocol matters for runtime implementers and SDK authors. Application
developers work at a higher level.

## Example: Echo Server

A minimal echo server:

```python
write("/iso/self/interface", {
    "paths": {
        "/{key}": {"read": "Echo the key", "write": "Store a value"}
    }
})

store = {}

while True:
    req = read("/iso/server/requests")

    if req is None:
        break

    if req.op == "read":
        value = store.get(req.path)
        write(req.respond_to, {"result": "ok", "value": value})

    elif req.op == "write":
        store[req.path] = req.data
        write(req.respond_to, {"result": "ok", "path": req.path})

write("/iso/shutdown/complete", {})
```

## Example: Async Job Processor

A job processor that returns handles:

```python
jobs = {}
next_id = 0

while True:
    req = read("/iso/server/requests")

    if req is None:
        break

    if req.op == "write" and req.path == "submit":
        # Accept job, return handle
        job_id = next_id
        next_id += 1
        jobs[job_id] = {"status": "pending", "input": req.data}
        write(req.respond_to, {"result": "ok", "path": f"outstanding/{job_id}"})

        # Process job (in real impl, this would be async)
        jobs[job_id] = {"status": "complete", "result": process(req.data)}

    elif req.op == "read" and req.path.startswith("outstanding/"):
        job_id = int(req.path.split("/")[1])
        job = jobs.get(job_id)
        if job and job["status"] == "complete":
            write(req.respond_to, {"result": "ok", "value": job})
        else:
            # Could block here until job completes
            write(req.respond_to, {"result": "ok", "value": job})
```

## Comparison to Other Protocols

| Protocol | Isotope Server Protocol |
|----------|------------------------|
| HTTP | Request/Response, but no verbs (just read/write) |
| 9P | Similar file server model, but with Values not bytes |
| gRPC | Request/Response, but path-addressed not method-addressed |
| Actor mailbox | Similar queue model, but typed as StructFS |

## Open Questions

1. **Request metadata**: Should Requests include caller identity, timestamps,
   or tracing context?

2. **Response streaming**: How should a Block stream a large response? Multiple
   Responses? Chunked values?

3. **Request cancellation**: Can a caller cancel a pending Request? How is the
   Block notified?

4. **Backpressure**: If Requests arrive faster than the Block processes them,
   what happens? Queue limits? Errors?

5. **Request priority**: Should there be priority levels for Requests?
