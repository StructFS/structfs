# StructFS Plans

Core principle: **Everything is a store.**

## Status

The initial plan and [consumer contract work](01-consumer-contracts.md) are complete.
The implemented 0.4 candidate is tracked in
[Coherent StructFS and Isotope contracts](02-coherent-contracts.md), covering value
construction, store conventions, recoverable execution, and consistent composition
without a backward-compatibility constraint.

See [final validation](../docs/release-validation-2026-09-15.md) for checks and artifact identities.

The [adoption-contract supplement](03-adoption-contracts.md) extends the 0.4
candidate with joined handle cleanup, explicit persistence acknowledgement and
recovery, streaming HTTP/SSE, names-only discovery, and structured codec diagnostics.

Current candidate evidence: [supplement follow-up audit](../docs/ox-supplement-audit-2026-09-16.md).
