# Why Blocks Have a Static Manifest

Blocks export a `manifest()` function that returns a JSON blob describing their
name, version, serialization format, and path interface. This document explains
why the manifest exists as a separate export rather than going through the store
interface.

## The Bootstrap Problem

Isotope's philosophy is "everything is a store"—all communication happens through
read/write on paths. Blocks self-describe at runtime by writing to
`/iso/self/interface`. So why not use the store for the manifest too?

Because the manifest declares the serialization format the store will speak.

When the runtime wires a Block, it must bridge the store boundary: the Block
speaks some encoding (JSON, CBOR, MessagePack), and the runtime needs a codec to
translate. The manifest is where the Block declares that encoding. But if the
Block can only declare it by writing through the store, and the store bridge
needs the declaration to be set up—that's circular. The runtime can't read a
message it doesn't yet know how to decode.

The manifest breaks this cycle by existing outside the store protocol, using a
pre-coordinated format (JSON) that both sides always understand.

## Why Not a Structured Type?

The manifest returns opaque bytes (`list<u8>`) rather than a structured type in
the host's type system (e.g., a WIT record). Three alternatives were considered:

### 1. Structured WIT record

WIT records would give type safety at the WASM boundary. But Blocks are not
exclusively WASM components—web is a target surface. Encoding the manifest schema
into WIT would marry a cross-runtime concern to the Component Model. A JS Block
running in a browser shouldn't need WIT tooling to produce a manifest.

### 2. WASM custom section

Custom sections can be read without instantiation, which would be cheaper. But
the same portability problem applies: custom sections are a WASM-specific
mechanism. Non-WASM runtimes would need a different path, bifurcating the
contract.

### 3. Self-description through the store

As described above, this creates a bootstrap cycle. The manifest must be
available before the store is wired.

## Why Always JSON?

JSON is the lingua franca. Every target runtime (WASM, web, native) can produce
and parse it without special tooling. The manifest is small, read once, and not
on any hot path—parsing cost is irrelevant.

This is deliberately independent of the Block's declared `serialization` field.
A Block that speaks `application/protobuf` for its store traffic still produces a
JSON manifest. One fixed format for the bootstrap handshake; the Block's chosen
format for everything after.

## Two Levels of Self-Description

Blocks describe themselves at two levels:

1. **Static manifest** (`manifest()` export): Available before wiring. Declares
   the serialization format and compile-time interface. The runtime uses this to
   select the codec and validate wiring.

2. **Runtime declaration** (write to `/iso/self/interface`): Goes through the
   store after wiring. Can include dynamic information not known at compile time.
   Tooling and other Blocks can read it through the normal store interface.

Both exist because they serve different moments in the Block lifecycle. The
static manifest is the linker's symbol table; the runtime declaration is
`/proc/self/maps`.
