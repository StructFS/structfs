# StructFS Code Review

*An honest assessment from the perspective of systems design and API architecture.*

---

## The Good

### The philosophy is right

"Everything is a store" with a uniform read/write interface on paths - this is exactly what Plan 9 did. The idea that mounts, HTTP requests, and system primitives all present as a filesystem-like interface is philosophically sound. Plan 9 had `/net` for networking, `/env` for environment, `/proc` for processes. StructFS does the same thing. That's good.

### The three-layer architecture is clean

- `ll-store` for raw bytes
- `core-store` for semantic paths and values
- `serde-store` for typed access

This is the right stratification. It pushes cost to the edges - raw byte protocols don't pay parsing costs, typed access doesn't deal with bytes.

### The OverlayStore trie-based routing is solid

Longest prefix matching, redirect support with cycle detection, proper fallthrough semantics. This is how mount tables should work.

### Testing is mature

93% coverage, dependency injection for HTTP mocking, TestHost for REPL testing. The test infrastructure shows maturity.

---

## The Concerns

### The HTTP broker pattern is clever but awkward

Write to queue a request, read to execute it - this inverts expected REST semantics. In REST, GET is idempotent and side-effect-free; POST has side effects. Here, read has side effects (executes HTTP calls). This will confuse anyone with REST intuition.

The deferred execution pattern is useful - the motivation is clear. But the naming lies about what's happening. Consider calling it something like "queue/execute" or "pending/resolve" rather than overloading read/write.

### `&mut self` on read is a red flag

The documentation justifies it, but this breaks the fundamental contract people expect from "read". Yes, some stores have side effects on read (HTTP broker, filesystem position). But forcing *all* readers to take `&mut self` because *some* need it solves the problem backwards.

In Plan 9, `/net/tcp/clone` didn't pretend to be just a read. The verb told you what to expect. Consider separate traits: `PureReader` (guaranteed no mutation) and `StatefulReader` (may mutate). Let the type system express the difference.

### The path validation is too restrictive

Only Unicode identifiers? No hyphens, no dots in filenames? This means:

- `/users/john-doe` is invalid
- `/config/.env` is invalid

Real paths have hyphens and dots. This wall will be hit immediately when mounting real filesystems.

The validation makes sense for *symbolic* paths (variable names, API endpoints). But not for *arbitrary* paths. Consider separating "path as identifier" from "path as filesystem location".

### The Value type reinvents JSON poorly

`Value::Bytes` exists, which JSON doesn't have, but JSON is the primary codec. So bytes get... what, base64 encoded? This will cause confusion. Either commit to a richer format (CBOR, MessagePack) as primary, or accept JSON's limitations.

### The REPL is doing too much

`commands.rs` is almost 2000 lines with manual ANSI color handling, register management, command parsing. This should be smaller. The help formatting alone is 500+ lines of match arms. Consider a declarative help format that gets rendered, not imperative code for each section.

---

## The Missing Pieces

### No streaming

Everything is `Record`, which is either bytes or Value. Where's streaming reads? Large files? Chunked responses? A store abstraction needs to handle unbounded data eventually.

### No metadata

Real filesystems have `stat()`. What's the schema of a path? Is it a leaf or a directory? What format is the data? `Format` hints exist but aren't exposed in the protocol. Consider a `stat` or `describe` operation alongside read/write.

### No atomicity

No transactions, no compare-and-swap, no "write if not exists". For a store abstraction to be useful beyond toys, atomic operations are necessary.

### No event subscription

Plan 9 had watch. Real systems need to know when things change. Read-poll is not sufficient.

---

## The Verdict

This is a thoughtful project with good foundations. The core idea is right. The implementation is clean. The testing is mature.

But it's trying to be too clever in places (HTTP broker semantics), too restrictive in others (path validation), and missing fundamental operations (streaming, metadata, atomicity) needed for real-world use.

The question is: what's this actually for? If it's a teaching tool or a unified scripting interface, it's probably fine. If people should build real systems on it, the gaps need addressing.

### Recommendations

1. Clean up the HTTP broker semantics - make the verbs honest
2. Loosen the path validation - support real filesystem paths
3. Add metadata operations - `stat` or `describe`
4. Consider streaming for unbounded data
5. Plan for atomicity primitives

Then this becomes something genuinely useful.
