# Isotope: A Virtual Operating System

Isotope is a virtual operating system built on StructFS semantics. It provides
a uniform computational model where all system interaction—process control,
inter-process communication, resource access—happens through read and write
operations on paths.

## Design Principles

### Everything is a Store

In traditional operating systems, there are many distinct interfaces: system
calls, signals, shared memory, pipes, sockets, files. Each has its own
semantics, error handling, and mental model.

Isotope has one interface: the Store. A Store accepts reads and writes on
paths. That's it. Process control? Write to a path. IPC? Read and write paths.
Configuration? Paths. Debugging? Paths.

This is not a new idea—Plan 9 showed that "everything is a file" dramatically
simplifies system design. Isotope pushes further: everything is a *store
operation*, and stores compose.

### StructFS as Transport

StructFS is a transport layer, not an application layer. Developers don't write
raw read/write handlers any more than web developers write raw TCP sockets.

A Block can wrap any application protocol:

- A gRPC service that thinks in protobuf and streaming RPCs
- An OpenAPI service that thinks in JSON and REST semantics
- A language-native SDK with decorators or traits

The wrapping layer translates between the application protocol and StructFS
operations. The Block's internal code never sees StructFS directly—it sees its
native protocol.

This spec defines the transport contract. How developers interact with that
transport is an SDK/framework concern, not a protocol concern.

### Blocks as Pico-Processes

The fundamental unit of execution in Isotope is the **Block**. A Block is:

- **Isolated**: It runs in a Wasm sandbox with its own memory
- **Single-threaded**: Like a goroutine or greenlet, but with a clear memory boundary
- **A Store server**: Each Block presents exactly one StructFS store to the outside world
- **A StructFS client**: Internally, a Block reads/writes paths in its namespace

Blocks are lighter than processes, lighter than threads. They're pico-processes
with an IPC discipline mediated entirely through StructFS. An Isotope system
might run thousands of Blocks where a traditional OS would run dozens of
processes.

### Assemblies as Composition

Blocks compose into **Assemblies**. An Assembly is itself a Block—it presents
the same interface to its parent as any leaf Block would.

An Assembly consists of:

1. **A set of Blocks** (which may themselves be Assemblies)
2. **A Public Block** that serves as the Assembly's external face
3. **Wiring** that connects Blocks internally

From outside, you cannot tell whether you're talking to a Block or an Assembly.
The fractal property holds: Assemblies contain Blocks (which may be Assemblies),
all the way down. The system itself is just the outermost Assembly.

### Location Transparency

A Block doesn't know—and cannot know—whether a path leads to:

- A value in local memory
- Another Block in the same Assembly
- A Block in a parent Assembly
- A Block on a remote machine
- A traditional file on disk
- An HTTP endpoint

This isn't just abstraction for abstraction's sake. It means:

- Testing: Replace any component with a mock by wiring differently
- Migration: Move Blocks between machines without changing their code
- Scaling: Fan out to multiple instances behind a single path
- Debugging: Interpose a tracing store at any wiring point

## Core Concepts

The Isotope specification defines:

1. **Block** — The execution primitive (see `01-blocks.md`)
2. **Assembly** — Block composition via Public Block + Wiring (see `02-assemblies.md`)
3. **Namespace** — Per-Block path visibility (see `03-namespaces.md`)
4. **System Paths** — Services at `/iso/` (see `04-system-paths.md`)
5. **Lifecycle** — Block states and transitions (see `05-lifecycle.md`)
6. **Protocol** — StructFS operation semantics (see `06-protocol.md`)
7. **Server Protocol** — How Blocks serve StructFS requests (see `07-server-protocol.md`)

## Non-Goals

Isotope does not specify:

- **Wire format**: How store operations are serialized for network transport
- **Execution engine**: Whether Blocks run as Wasm, native code, or interpreted
- **Scheduling**: How Blocks are scheduled for execution
- **Persistence**: How state survives restarts

These are implementation concerns. Different Isotope runtimes may make different
choices. Featherweight, for example, uses Wasm via wasmtime.

## Relationship to StructFS

StructFS defines the core abstractions: Path, Value, Record, Reader, Writer.
StructFS also defines patterns like References, the Meta lens, and Pagination.

Isotope builds an operating system model on top of these abstractions. If
StructFS is the "filesystem and API interface," Isotope is the "operating system
that uses that interface for everything."

Critically, the **Server Protocol** that allows Blocks to serve StructFS stores
is defined relative to StructFS itself—a Block is simultaneously a StructFS
client (reading requests, writing responses) and a StructFS server (handling
operations from other Blocks).
