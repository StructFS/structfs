# APIs hard, especially if you don't have the words

This document discusses the ways that StructFS empowers encoding of higher order
systems within it's narrow waist of representations.  We will discuss the
hierarchy of encoding, but first let's consider the words we will use.

StructFS defines some key terms to make API discussion easier and more
consistent:

* **name** \- a single well-known string representing an addressable unit of
  data
* **struct** \- a tree-shaped data structure read from or written to a StructFS
  Store.  Think C struct, JSON object,
  [dataclasses](https://web.archive.org/web/20210217113801/https://www.gidware.com/useful-data-classes/) (like those in
  Kotlin, Python's dataclasses, Java's POJOs, etc.), etc.
* **field datatype** \- a well-known representation of data that can be used to
  understand what memory layout will be, what data a field can contain, etc.
  within some known context.
* **schema** \- a constrained structure of the collection of fields and
  datatypes a struct may contain.
* **path** \- a well defined series of field names that describes a path of
  lookup through a struct.
* **access tree** \- the term for a tree of StructFS Stores accessible from some
  system using StructFS.  An access tree should be thought of as just another
  Store.  In this case an access tree is a \_composite\_ of other Stores.
* **namespace** \- the path domain of an access tree.  Think of a namespace as
  being all the paths provided by some given Store and the access tree as the
  interactive structure the Store represents.
* **action** \- read or write -- the fundamental units of behavior encoding
  within StructFS.
* **interface** \- a promise of read/write structures provided by a Store.  An
  interface can be considered a static set of affordances provided by the Store.
  Think of an interface as a *schema over verbs*.
* **protocol** \- a state machine defining some serializable interaction with a
  Store's interfaces.  It is the goal of StructFS to unify various common
  protocols over time to create a set of "common drivers" for reusable internet
  applications components.  Think of a protocol as *an interface over time*.

## API complexity and higher-kinded APIs

So, we can think of the most fundamental layer as that of non-struct *field
datatypes*.  These define some simple set of values that may be witnessed by
some name.  For our purposes, primitive types here include things like numbers,
strings, byte strings, etc.  Arrays are a composite non-struct field datatype
that may contain an arbitrary number of values of heterogeneous datatype.  For
people used to homogeneous array types, this is to better map to encodings
present in dynamic languages and the most common serialization formats: JSON and
XML.  Schema (to be described later) may be used to constrain the values
obtainable to a homogeneous domain to better constrain properties expected by
languages that expect this shape.

> A *value* may be of some *field datatype*.

From there we may construct *structs*.  A struct is a mapping from field names
to datatypes.  Note that StructFS defaults to unschematized / fully unstructured
data.  Schema constraints must be enforced at the application layer (or
delegated to by a validation/enforcement layer running on behalf of the
application).

> A *struct* is a composite named map of *values*.

Structs may be combined with *actions* to compose an interface.  Think of an
interface in Go, Java, or TypeScript terms with a minor twist: there's no
distinction between data stored in a field and the read action performed on a
name for that struct.  Similar to Smalltalk, Kotlin, Python, etc. which have
overridable / implicitly logical getters, there is no way for a consumer of a
store to distinguish between a constant field and a "getter" over a constant.

> An *interface* is a *struct* whose fields are represented by read/write
> *action* pairs.

Actual applications need to perform actions in well-defined orders to achieve
goals.  We call our schematized form of this action state machine a *protocol*.
Protocols define an abstract *struct*-based state machine over some set of
*actions* over some set of *interfaces*.

> A *protocol* is functional an *interface* "through time" -- we model a
> *protocol's* state machine as an abstract *struct*-based state model and the
> set of *actions* as transition edges over a family of *interfaces*.

## Recommended StructFS API properties

### Consistency

A StructFS store is considered strongly consistent if reading /foo returns a
structure which contains a field "bar", and reading /foo/bar returns an
identical value for all potential field nestings within that store.  A JSON
object accessed via StructFS for path selection would be strongly consistent.

This consistency, however, isn't always what we _want_ from an API.  Sometimes
we're encoding an external system that doesn't _have_ a concept of fields,
recursive consistency, temporal stability, etc.  Thus it is valuable to consider
consistency as a property dimension of API "texture" without giving value
judgement.

> When given two equal decisions when defining a StructFS API, prefer the more
consistent one.

### Concision

A StructFS interface is concise if there is one and only one way to perform a
given action.
