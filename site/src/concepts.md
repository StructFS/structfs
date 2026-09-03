---
layout: base.njk
title: The Model
permalink: /concepts-md/
---

# The Model

::: concept-prose
StructFS is an architectural style for data access.
{.concept-lead}

It defines a uniform interface (two operations over paths and structured values) and a composition model where anything implementing that interface is a *store*, and stores mount into a shared namespace tree.
{.concept-detail}
:::

## The store {#the-store}

::: concept-prose
A store is anything that responds to read and write.
{.concept-lead}

A tree of data in memory is a store. So is an HTTP endpoint, a system clock, a database, or a random number generator. The differences between these systems (protocol, client library, error handling, testing story) are real, but they're accidental. At the level that matters to the application, they're all the same thing: data behind an address.
{.concept-detail}

StructFS makes that sameness the foundation. A store is a store is a store.
{.concept-detail}
:::

## Two operations {#two-operations}

::: concept-prose
Every store responds to the same two operations. That's the entire interface.
{.concept-lead}
:::

:::: concept-ops
::: concept-op
### read(path) <span class="op-returns">→ value | none | error</span>

Returns the value at that path. The path may not exist (none), or the operation may fail (error). Callers handle both cases.
:::

::: concept-op
### write(path, value) <span class="op-returns">→ path | error</span>

Sends a value to that path. On success, returns a result path, often the same path, sometimes a new one. On failure, returns an error.
:::
::::

::: concept-prose
No DELETE, no PATCH, no LIST, no SUBSCRIBE. This seems like a limitation until you see why it isn't: **paths carry meaning**.
{.concept-detail}

HTTP needs many verbs because URLs are nouns: you need different verbs to do different things to the same noun. In StructFS, a path can name an action just as easily as it names a resource. Writing to `users/alice/deactivate` is as expressive as `DELETE /users/alice`, but the deactivation is itself a path, one that can be composed, proxied, or intercepted with the same mechanisms as everything else.
{.concept-detail}

Two operations isn't a constraint that limits what you can express. It's a constraint that forces expressiveness into the one place where it composes: the path.
{.concept-detail}
:::

## The tree {#the-tree}

::: concept-prose
Stores compose into a tree.
{.concept-lead}

Mount a store at a path prefix, and all reads and writes under that prefix route to that store. Mount several stores at different prefixes, and you have a unified namespace:
{.concept-detail}
:::

::: concept-tree
/<br>
├── users/ <span class="tree-label">← database store</span><br>
├── api/ <span class="tree-label">← HTTP client</span><br>
├── cache/ <span class="tree-label">← in-memory store</span><br>
└── config/ <span class="tree-label">← read-only store</span>
:::

::: concept-prose
A store doesn't know where it lives in the tree. When you read `/api/items`, the HTTP store sees `items`, not `/api/items`. This is what makes stores swappable: attach a mock at `/api` instead of the real client, and nothing else changes.
{.concept-detail}

The tree is the wiring diagram of your system. You can rewire it without touching application code.
{.concept-detail}
:::

## Write returns a path {#write-returns-a-path}

::: concept-prose
This is the mechanism that makes two operations sufficient for complex systems.
{.concept-lead}

When you write to a store, the return value isn't an acknowledgement; it's a *path*. That path might be the same one you wrote to, confirming the write. Or it might be a new path, a handle you can read from later.
{.concept-detail}
:::

::: concept-exchange
<span class="ex-prompt">write</span> /jobs {"task": "resize", "input": "photo.jpg"}<br>
<span class="ex-result">→ /jobs/pending/0</span><br>
<br>
<span class="ex-prompt">read</span> /jobs/pending/0<br>
<span class="ex-result">→ {"status": "complete", "output": "photo_resized.jpg"}</span>
:::

::: concept-prose
The write queued work. The read collected the result. The interface stays synchronous (no callbacks, no futures, no event loops) but the interaction is multi-step.
{.concept-detail}

The same pattern models file handles, subscriptions, sessions, transactions: any interaction that spans multiple operations. You don't need `OPEN`, `CLOSE`, `SUBSCRIBE`, `EXECUTE`. Each is just a path you read from or write to.
{.concept-detail}
:::

## Structured values {#structured-values}

::: concept-prose
Classic filesystems deal in bytes. Infinite flexibility, zero interoperability.
{.concept-lead}

StructFS deals in structured values: the same types your programming language already gives you, and the same model that every serialization format converges on. JSON, CBOR, MessagePack, Protocol Buffers: they all encode the same core shapes. StructFS makes that shared model native. When a store returns a value, it's already structured. When you write a value, there's no encoding step. The structure *is* the data.
{.concept-detail}

This is the departure from Plan 9. 9P gives you a universal namespace over byte streams; you still need to agree on a wire format between every producer and consumer. StructFS gives you a universal namespace over structured data. The serialization format is an implementation detail of the transport, not a concern of the interface.
{.concept-detail}
:::

## Path encoding {#path-encoding}

::: concept-prose
Structured values solve the data boundary. Paths need the same treatment.
{.concept-lead}

A path component in StructFS must be a valid identifier in every language that might handle it: Rust, Go, JavaScript, Python, and anything else that touches the tree. This is the same problem HTTP faces with URLs: arbitrary strings need to be constrained to a form that survives every context they pass through.
{.concept-detail}

HTTP's answer is percent-encoding. StructFS's answer is [Namecode](https://namecode.dev), an encoding that turns any Unicode string into a valid [UAX 31](https://unicode.org/reports/tr31/) identifier. Think Punycode for variable names.
{.concept-detail}
:::

::: concept-exchange
foo <span class="ex-result">→ foo</span> <span class="ex-result" style="margin-left: 1rem;">valid identifier, passes through</span><br>
café <span class="ex-result">→ café</span> <span class="ex-result" style="margin-left: 1rem;">Unicode identifiers pass through too</span><br>
hello world <span class="ex-result">→ _N_helloworld__fa0b</span> <span class="ex-result" style="margin-left: 1rem;">space encoded</span><br>
foo-bar <span class="ex-result">→ _N_foobar__da1d</span> <span class="ex-result" style="margin-left: 1rem;">hyphen encoded</span>
:::

::: concept-prose
The encoding is reversible, deterministic, and idempotent. Strings that are already valid identifiers pass through unchanged; most path components in practice never get encoded at all. But when a path component comes from user input, an external system, or any source that might contain spaces, punctuation, or emoji, namecode guarantees it survives the round trip through every language boundary in the system.
{.concept-detail}

This is the same role URL encoding plays for REST. Without it, HTTP URLs would break the moment they crossed a context boundary. Namecode gives StructFS paths the same guarantee: any path, any language, any store.
{.concept-detail}
:::

## The representation hierarchy {#the-representation-hierarchy}

::: concept-prose
Two operations and structured values. How does this scale to real systems?
{.concept-lead}

Through conventions at increasing levels of abstraction:
{.concept-detail}
:::

:::: hierarchy-stack
::: hierarchy-level
Values
{.hierarchy-label}

A path names a value: a string, a number, a map. Reading and writing at this level is direct data access.
{.hierarchy-desc}
:::

::: hierarchy-level
Structs
{.hierarchy-label}

A tree of named values. Read a parent, get a map. Read a child, get a field. Paths decompose into structure.
{.hierarchy-desc}
:::

::: hierarchy-level
Interfaces
{.hierarchy-label}

A struct whose fields are backed by behavior. There's no distinction between a stored field and a computed one; a consumer can't tell. An interface is a schema over actions.
{.hierarchy-desc}
:::

::: hierarchy-level
Protocols
{.hierarchy-label}

An interface through time. A state machine of reads and writes against a family of paths. Handles, brokers, pagination: all protocols. The state machine lives in the path structure and the ordering of operations.
{.hierarchy-desc}
:::
::::

::: concept-prose
Each level builds on the one below without new operations or new primitives, just conventions about what paths mean and how they relate. A store that implements a complex protocol is still a store that responds to read and write.
{.concept-detail}

This is why two operations are sufficient. The complexity lives in the paths and the values, not in the verb set.
{.concept-detail}
:::

## Design lineage {#design-lineage}

::: concept-prose
StructFS sits at an intersection of three traditions.
{.concept-lead}
:::

:::: lineage-stack
::: lineage-item
### Plan 9 and 9P

StructFS inherits the namespace philosophy: everything accessible through paths, composition through mounting. The departure is the data model: 9P traffics in byte streams, StructFS in structured values. This eliminates the serialization boundary between components. And StructFS is a library-level abstraction, not a kernel facility.
:::

::: lineage-item
### REST

Both define an architectural style around a uniform interface. REST has four operations over representations; StructFS has two over structured values. REST puts semantics in verbs: different verbs for different actions on the same resource. StructFS puts semantics in paths: different paths for different actions, same two verbs. REST constrains network interaction; StructFS constrains data access at any layer.
:::

::: lineage-item
### Capability systems

A StructFS path is a capability: possession of the path grants the ability to read or write. The tree is a capability graph; mounting grants access to a subtree, and relative addressing means a store can't reach outside what it was given. This is structural, not an access control layer bolted on.
:::
::::

<nav class="page-nav">
  <a href="/stores/" class="page-nav-next">Continue to Stores →</a>
</nav>
