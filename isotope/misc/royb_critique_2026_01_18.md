# Isotope Specification Critique

A review of the Isotope spec from a Plan 9 / REST perspective.

---

## The Good

**You've understood Plan 9.** That's not nothing. Most people who cite Plan 9 don't actually get it. They think "everything is a file" was about files. It wasn't. It was about protocol uniformity. You've grasped that. The Server Protocol spec where Blocks read Requests from `/iso/server/requests` and write Responses—that's 9P thinking applied correctly. The Block doesn't know it's a server. It just reads and writes paths. That's the insight.

**Namespace isolation is correct.** The capability model where a Block can only see paths wired to it—this is right. The `/iso/` reserved prefix is right. Path rewriting at mount boundaries is right. You've avoided the common mistake of giving processes global visibility.

**The fractal Assembly property is elegant.** Assemblies are Blocks. This is good. It means your testing story writes itself, your composition is uniform, your mental model scales. The explicit wiring in YAML means the architecture IS the config. No lies between diagram and deployment.

**Deadlocks are acknowledged.** Spec 07 says deadlocks are possible and the runtime must detect them. Most people designing message-passing systems handwave this. You didn't.

---

## The Concerns

**You've reinvented actors, but with more ceremony.** The Block model is Erlang processes with a path-based mailbox. That's fine. But Erlang doesn't require you to write a run loop in every process. You have to implement the request dispatch yourself every time. That's boilerplate that will become a source of bugs.

Consider whether there should be a declarative Block interface—you write handlers, the runtime dispatches. The current model gives maximum flexibility at the cost of maximum repetition.

**The "Value" type system is underspecified.** You say stores exchange Maps, Arrays, Strings, etc. But you haven't said how these are serialized, how schema validation works, what happens when types don't match at runtime. The spec says "type_mismatch" is an error type, but doesn't say how types are declared or checked.

This will bite you. Either you need something like Protocol Buffers (explicit schemas) or you need to accept dynamic typing with all its consequences. Right now you're in a middle ground that satisfies neither.

**Blocking semantics are hand-wavy.** The protocol spec says reads can block. The Server Protocol says `/iso/server/requests` blocks. But there's no specification of:
- Timeout behavior
- Cancellation
- What "blocking" means in a single-threaded Wasm sandbox

If a Block blocks reading from another Block that blocks reading from the first Block, you have a deadlock. The spec acknowledges this but the primitives to avoid it (non-blocking reads, handles) feel like afterthoughts rather than first-class citizens.

**The handle pattern needs formalization.** You mention it in Protocol spec line 90 and Server Protocol spec line 233. It's clearly important—it's how you do async. But it's a pattern, not a primitive. What makes a path a "handle"? How does a client know to poll vs block? Can handles expire? Be invalidated?

If handles are central to avoiding deadlocks, they should be in the type system, not just the documentation.

**Error abstraction is aspirational.** The Protocol spec says errors must not leak implementation details—"unavailable" not "cache block crashed." That's the right principle. But your example Blocks all return raw implementation errors. There's no mechanism to enforce error abstraction at Assembly boundaries.

**The lazy startup story has holes.** Blocks start on first access. But what if a Block needs its dependencies ready before it can serve? The "poke" pattern (`write("/services/cache/ping", {})`) is ad-hoc. A startup ordering mechanism (or explicit readiness signaling) would be cleaner.

---

## The Harder Questions

**What's the unit of deployment?** An Assembly is a Block, sure. But when you say "deploy to production," what's actually happening? A Wasm runtime starts somewhere and loads an Assembly definition. But who manages that runtime? How do Assemblies span machines? The spec explicitly punts on this ("wire format" and "execution engine" are non-goals), but it leaves the hardest problem unsolved.

**Where's the network?** For location transparency to work, path operations need to cross machine boundaries. But there's no specification of how that happens. You can't just mount `remote://other-machine/service` without defining what that means. 9P defined the wire protocol. You haven't.

**State and persistence are missing.** Blocks have state (the examples show `store = {}` dictionaries). When a Block restarts, that state is gone. The spec says checkpointing is out of scope, but then how do you build anything stateful? The answer is presumably "external database Block," but that just pushes the problem elsewhere.

---

## Verdict

This is a thoughtful design. You've absorbed the right lessons from Plan 9, Erlang, and capability systems. The Store abstraction is sound. The composition model via Assemblies is good.

But it's a *partial* design. You've specified the compute model well, but punted on:
- Wire protocol for distribution
- Type system for Values
- Formal handle/async semantics
- Persistence model

These aren't nitpicks—they're the iceberg under the water. The current spec would let you build a nice in-process component framework. To build an actual operating system, you need the rest.

The rationale docs are good. "Empowering Application Engineers" is a clear vision statement. The question is whether the spec delivers on it. Right now, an app engineer reading this spec still can't build a real service without filling in the gaps you've left unspecified.

**Recommendation:** Pick one of the hard problems—probably wire protocol or handle semantics—and nail it. A deep spec of one thing beats a shallow spec of everything.
