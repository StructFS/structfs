# StructFS

Modern application developers want:

* Structures instead of bytes
* Composable interfaces instead of monolithic ones
* Portable systems

In today's world of digital expansion, a majority of new software is internet
connected.  By extension, a significant portion of new APIs span the network
boundary.

In response to the challenging environment of networked and distributed
services, various dogma has come to thrive and battle for mindshare.  Should you
"use a microservices architecture"?  What about a stream processing service?
What API protocols should you use?  Should your teams standardize on one
protocol like gRPC or let each team make their own decisions?

With networked software, this problem gets exacerbated by the crisis of choice
among languages (should we use Python, go, Rust, JavaScript, etc.), frameworks /
runtimes (Django Rest Framework vs. Flask vs. FastAPI, Node.js vs. Deno vs. Bao,
Axum vs. Warp vs. Rocket, etc.).

In the end there's no "right" decision except to "use what is productive for
you".

For us, this slow-motion explosion of choice has left a bad taste in our mouths.
Whenever a given "stack" is chosen, why does the DevOps burden have to scale
quadratically?  What if there were a baseline lingua-franca that shares more
between these systems than TCP/IP?

This is why we built StructFS.

StructFS is a virtual Filesystem-like API system (we'll get into what exactly
this means below).  StructFS combines the simplicity and composability of a
virtual filesystem with the application-oriented structure of modern API
protocols like ReST-ish HTTP+JSON, gRPC, or GraphQL.

StructFS is agnostic to serialization formats (as ReSTful APIs \_should\_ be),
inherently provides capability control (like 9p and other virtual filesystem
protocols) through recursive composition, and is designed for cross-language
interoperation.

So what \_is\_ StructFS and how can you use it?

# Comparisons

To understand StructFS, let's start by comparing it to systems you may be
familiar with.  Note that, while we're comparing the details of these systems
vs. StructFS, the ultimate goal is to provide fully virtualized interfaces
\_to\_ these systems through StructFS. I.e. eventually you'll be able to access
any filesystem or ReST API without leaving the cozy confines of your StructFS
access tree.

## The POSIX filesystem vs. StructFS

* "Classic" filesystems deal in bytes.  While this provides infinite flexibility
  in how files are structured and what they can store, it forces every
  application to reinvent the wheel.  The rise of APIs structured using default
  application-oriented formats (especially JSON) has demonstrated that modern
  teams prefer productivity and interop over low-level control and performance.
  StructFS deals in "structs" which have a consistent semantics across
  languages.  No more writing file format and network protocol parsers,
  everything is presented as structs from the get go.
* StructFS preserves the beauty of file paths and recursive access schemes.
  Instead of folders vs. files, however, paths are unified across all data.  To
  access JPEG EXIF headers in random.jpg, an application would access
  path/to/random.jpg/exif (i.e. the exif field on random.jpg).  Think of this
  like having one unified data tree for \_everything\_ in the StructFS system.
  We call this pattern of access a Recursive Record Store.
* All Stores are virtual stores, unlike classic filesystems which try to make
  any virtualized API look like a local disk (often failing to provide
  sufficient abstraction \*cough\* NFS \*cough\*). This means that storage
  systems providing a StructFS Store interface can cleanly integrate various
  backends without the application having to contort to various constraints
  around filesystem availability and failures.

## ReSTful HTTP+JSON vs. StructFS

* Both ReSTful APIs and StructFS have the concept of format agnosticism.  If you
  want StructFS+JSON, that's supported out of the box.
* HTTP provides a plethora of verbs such as POST, PUT, DELETE, GET, etc.
  StructFS provides two methods: read and write.  These mostly mirror the read
  and write operations present in the filesystem interface, but they have a
  minor twist: read and write manipulate the resource tree much in the same way
  that HTTP verbs do.  To write the EXIF data for a JPEG, you might write to
  some path/to/random.jpg/exif resource.
* HTTP nouns and StructFS resources are both represented as URIs.  StructFS URIs
  have an even more constrained encoding than HTTP ones: all symbols must
  fulfill the Unicode Identifier Standard plus underscore.  This means, like
  URIs, an encoding/decoding step is necessary for bytes/characters outside the
  encoding space, but it provides an added benefit: StructFS paths \_are
  representable as native variables/symbols in a majority of languages\_ (if the
  language supports Unicode variable names, then StructFS paths should be
  compatible).  This sleight of hand let's many StructFS libraries provide a
  well-typed interface instead of having to always pass around string-like path
  types.

## GraphQL vs. StructFS

* Like GraphQL, StructFS can be used to wrap existing APIs in a unified
  interface.
* Unlike GraphQL, StructFS is not opinionated about filtering.  There is no
  explicit "only these fields" syntax.  This allows StructFS to be used as an
  intermediate proxy / processing representation whereas GraphQL must be
  relegated to only API-graph edge use-cases.

# The StructFS *base calls*

## read(path) \-\> struct *or* error

Read returns the value of the provided path within the interrogated Store.

The bottom type (e.g. null, nil, none, etc.) is idiomatically provided if that
path does not exist and it was not an error to request it.  Note that this means
a path returning "null" because that path isn't set is indistinguishable from
being explicitly set to null.

This default null return reflects a desire in StructFS for simplicity \_and\_
power.  Many modern languages and serialization formats (of which go,
JavaScript, JSON, and proto3 are all examples) have similar or identical
behavior.  This allows encoding that native behavior in interacting StructFS
systems.  If your system should provide a clear error upon a path not existing,
that is perfectly reasonable, but not canonical.  Do what is best for your
application.

## write(path, struct) \-\> path *or* error

Write sets the provided path to the provided struct.

This may be considered akin to:

* a POST in HTTP with the struct representing the payload and the path
  representing the URI
* a write to a file in Plan9 or similar UNIX-like system which provides virtual
  filesystems

The returned path represents a URI result of the write transaction.  It may be a
self contained result resource which can be directly interpreted, a resource to
read a response to, or a resource to continue writing to, etc.  Use this path to
best encode your APIs for readability and ergonomics.

One way this return path is used in StructFS protocols is to manage asynchronous
operations.  Both read and write are 100% synchronous, but that doesn't mean any
effects have to be.  Imagine a Store which interfaces with a work queuing
system.  When you write a new task struct to a queue path, the returned path may
represent a handle for interrogating the status of the work to be done.  This
way, StructFS remains simple (e.g. fully synchronous across the StructFS
interface boundary itself) while empowering an effective (and ergonomic) way to
model asynchronous behavior.

# Example API "Encodings"

If you would like to encode a blocking protocol (think, sleeping on a mutex
until it's free or awaiting an HTTP response) into StructFS, the Store should
provide a read/write (whichever best expresses the action being taken) which
blocks until the action has been resolved.

Note that systems which allow some agent to be both provider and consumer of
stores must consider the case where that agent is blocking when an incoming
transaction is incident.  Canonical StructFS implementations return an error in
all cases where this occurs to avoid potential deadlocks and complexity
introduced by any naive queueing.  If queuing is preferred, then the system
should encode that queuing in its design as domain knowledge can then be used to
avoid concurrency failures.

# The Driver System for modern Internet Applications

Imagine you're an application developer in the 1970s.  You're building a
word-processor application and want to print to a connected dot-matrix printer.
How do you communicate with it?  Well... that would depend on what OS you're on
top of and whether or not there's a driver for that operating system.

Thinking back with the perspective of today, it feels absurd.  Why not have a
plug-and-play system that just lets you attach any printer to your Mac, Windows,
or Linux PC and print?  Well, it required time and perspective on what printers
actually did and how applications needed to talk to them before standardization
could occur.

That's where we are today with the modern internet: a myriad of API encodings
(ReSTful HTTP+JSON, gRPC, GraphQL, etc.) on a myriad of devices (Desktop
(Windows, MacOS, Linux, etc.), Mobile (Android, iOS, etc.), Cloud, iOT devices,
the list goes on) with a myriad of use-cases.

What if we had a unified intermediate representation for APIs?  One which was
low-level enough to encode things in-process, but could be mapped in a clean way
to both application-level representations and underlying network/filesystem/OS
protocols?

That's the goal of StructFS: to provide a clean driver interface layer that
doesn't encode the incidental complexity of any particular encoding scheme or
I/O.  If you're familiar with the [sans-IO](https://sans-io.readthedocs.io/)
philosophy, this may sound familiar: build the protocol by itself first to
improve testability, reusability, and extensibility, then integrate it into the
environment it will run.

An application which uses StructFS to interact with a ReSTful API service can be
easily wrapped \_without mocking the network\_ for use in a completely different
application than originally intended: just provide a thin translation from the
StructFS encoding of HTTP to your API of choice, and voila\!

## Properties

* StructFS is simple.  Use an off-the-shelf client implementation (recommended)
  or roll your own if you have a novel use-case not yet supported.  If you do
  roll your own, please share it with the community so that we may all benefit.
* StructFS is transport agnostic.  It can model interfacing with filesystems,
  network protocols, application APIs, what-have-you in a lightweight and
  portable way.
* StructFS is data-interchange agnostic.  Use JSON, CBOR, Protobuf, Flatbuffers,
  or whatever makes sense to you.
* StructFS is language agnostic.  Use it from any language.  If you wrote your
  own language, provide a StructFS implementation so you can interoperate with
  other StructFS-compatible systems\!

## StructFS API Design Principles

* Explicit over implicit: where other protocols would implicitly encode
  information in-band with some existing property, a StructFS API should
  explicitly encode that information.  E.g. file suffixes implicitly communicate
  the format of the underlying file.  One could rename foo.txt (containing
  freeform text) to foo.jpg and the filesystem would be none-the-wiser but
  applications reading it might show incorrect data.  With StructFS, this "pure
  string" structure of a file is explicitly communicated by the content of the
  file being a string-typed field.  In cases where types differ only by name
  (and not by structure), a filesystem surfaced through StructFS could go a step
  further, offering a "meta/" lens for reading metadata about the file and
  providing an explicitly named schema which differentiates, say, a JAR from an
  APK.  Instead of worrying about cramming type information into a file
  extension, the metadata can contain a full MIME type for easy human (and
  computer) readability.

# Terminology

* **Store** \- a virtual access tree within StructFS.  Implement your systems as
  StructFS Stores and surface them to clients.
* **Server** \- a system that provides a Store for clients to access
* **Client** \- a system which accesses a StructFS tree of attached Stores
* **attach/detach** \- verbs for making a Store accessible or removing it from
  an access tree
* **access tree** \- the term for a tree of StructFS Stores accessible from some
  system using StructFS.  An access tree should be thought of as just another
  Store.  In this case an access tree is a \_composite\_ of other Stores.
* **struct** \- a tree-shaped data structure read from or written to a StructFS
  Store.  Think C struct, JSON object,
  [dataclasses](https://web.archive.org/web/20210217113801/https://www.gidware.com/useful-data-classes/) (like those in
  Kotlin, Python, Java's POJOs, etc.), etc.
* **schema** \- a constrained structure of a struct.
* **interface** \- a promise of read/write structures provided by a Store.  An
  interface can be considered a static set of affordances provided by the Store.
  Think of an interface as a *schema over verbs*.
* **protocol** \- a state machine defining some serializable interaction with a
  Store's interfaces.  It is the goal of StructFS to unify various common
  protocols over time to create a set of "common drivers" for reusable internet
  applications components.  Think of a protocol as *an interface over time*.
* **affordance** \- some functionality provided by a Store.
* **lens** \- a virtual prefix over a shared namespace which provides added
  functionality.  Reading a field directly might return its value, but
  prefixing it with meta/ might return last access time, etc.
* **namespace** \- the path domain of an access tree.  Think of a namespace as
  being all the paths provided by some given Store and the access tree as the
  interactive structure the Store represents.
