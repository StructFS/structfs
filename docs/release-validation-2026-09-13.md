# Consumer contract validation — 2026-09-13

The checkout targets **0.3.0, unreleased**. The Ox feedback work is tracked in
[consumer contract completion](../plans/01-consumer-contracts.md), with API and
compatibility decisions in [the migration guide](migration-0.3.md).

Validated locally on macOS with Rust 1.96.0:

| Check | Result |
| --- | --- |
| `cargo test --workspace --all-features --locked --offline` | 1,347 passed, 0 failed, 25 ignored |
| Workspace Clippy, all features and targets, warnings denied | Passed |
| Workspace documentation, all features, warnings denied | Passed |
| Workspace and external consumer formatting | Passed |
| Release-driver Python regressions | 12 passed |
| Each facade feature independently selected, including no defaults | Passed |
| Independent portable consumer, `wasm32-unknown-unknown` | Passed |
| Independent portable consumer with native HTTP features | Passed |
| External embedding lifecycle regression and strict Clippy | Passed |
| External application consumer checks with synchronized lockfiles | Passed |
| `scripts/check-featherweight-release.sh` | Passed; 690 tests passed, 0 failed, 17 ignored across its test invocations |

The archive gate packages every publishable workspace crate, extracts the actual
archives, and builds them with external consumers. It now additionally runs the
HTTP lifecycle example, release-mode path construction checks, detached contract
regressions, and the portable consumer's browser/native configurations. The
existing guest feature builds, codec reference validation, profiles checks and
strict extracted-archive documentation also pass.

The lifecycle fixture demonstrates a real loopback client disconnect after an
external allocation is accepted. A late reply still reaches ownership; cleanup
joins the producer independently of public path aliases. Engine and supervisor
capacity stay charged while work is pending or cleanup has failed. Failed cleanup
is acknowledged only after explicit fixture evidence that the resource terminated.
The same prepared guest is reused and its nonzero exit is observed separately
from optional trap diagnostics.

Native HTTP initialization and loopback listeners require execution outside the
restricted macOS sandbox. The unrestricted runs above passed. Linux CI and browser
JavaScript runtime tests are separate checks; local wasm compilation alone is not
a browser runtime certification. No publication or tagging was performed.
