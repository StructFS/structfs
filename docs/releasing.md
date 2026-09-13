# Coordinated release procedure

StructFS and Featherweight currently target 0.2.0; Isotope accompanies them as
specification snapshot 2026-09-12. Namecode is independently versioned. Treat these
as candidate versions until exact registry lookup succeeds. A previously published
incompatible 0.2.x contract requires a 0.3.0 release and synchronized dependency and
documentation updates. Never overwrite an existing publication.

Registry preflight on 2026-09-12 found the candidate versions unpublished and
Namecode 0.1.1 already published. StructFS's registry history contains 0.1.0.
Repeat preflight immediately before publication; this observation is not a lock
on registry state.

See [local release evidence](release-validation-2026-09-12.md) for the completed
checks, measured capacity workload and remaining environment-specific acceptance.

## Candidate verification

Use Rust 1.96+, Python 3.12 (archive extraction), and cached Cargo dependencies:

```sh
cargo fetch --locked
rustup target add wasm32-unknown-unknown
scripts/release.sh --plan
scripts/check-release.sh
```

The plan is read-only and offline. The gate runs release-driver regressions,
formatting (including external consumers), all-feature workspace tests and Clippy,
strict workspace docs, isolated facade feature checks, and actual extracted-archive
consumer verification. The archive gate checks guest feature modes and portable
profiles. It never publishes or tags. On macOS the HTTP tests need ordinary
SystemConfiguration access, so a restricted execution sandbox may prevent them
from initializing.

In `featherweight/host/browser`, run `npm ci`, `npm run check`, and `npm test` using
Node 25.9.0. CI runs these separately, plus the native gate on Linux/macOS with
Rust 1.96 and stable. All PRs trigger the workflow, including spec/fixture changes.
Configure branch protection to require these jobs; workflow files alone cannot
set repository protection. Windows and actual browser engines are not certified.

Review changelog, migration guide, spec matrix, packaged README links and ignored
tests. The three ignored runtime tests regenerate fixtures or print scheduling
diagnostics; ignored documentation snippets are illustrative and are not executed
acceptance tests. The 10,000-session capacity harness is an explicit additional
performance run, not a substitute for latency/throughput measurements of a real
production provider. Record hardware, toolchain, workload, latency and memory
units with any published performance claim.

Run a downstream application's migration tests using extracted archives. The four
independent consumers are the repository's acceptance fixtures; Ox/Horns integration
requires those applications' own environments and tests.

## Registry preflight and publication

```sh
scripts/release.sh --dry-run
```

Preflight queries exact sparse-index versions and fails on registry errors. Only
HTTP 404 means no crate exists. A local tag never proves registry publication.
Dry-run runs the complete archive gate rather than individually dry-running crates
whose new sibling dependencies are not published yet. `--skip-gates` is only for
an unchanged tree whose complete gates already passed.

`--crate NAME` and `--exclude NAME` are repeatable. Selection does not silently add
other crates. Dependencies omitted from the plan must have their exact local candidate version
already published and unyanked; Cargo verifies resolution again before uploading
each package. Review the full
plan when coordinated dependency versions change.

After the candidate passes CI and review, commit it on main with a clean tree.
Actual publication is a separate, deliberate action:

```sh
scripts/release.sh
```

Cargo uses the configured registry credentials. The driver publishes in dependency
order, waits for the exact version to become visible, then creates its annotated
crate tag. `--propagation-delay` is the maximum visibility wait in seconds, not a
blind sleep. The driver does not push tags or publish websites.

When crates.io rejects an upload with HTTP 429 and a recognized "Please try
again after" UTC date, the driver waits until that date (plus two seconds), then
retries the same crate. It prints progress every 30 seconds and checks that HEAD
and the working tree are unchanged before each attempt. Retries are limited to
three per crate and one hour per wait. Unknown retry formats, longer waits and
other failures still abort; ambiguous upload failures are not retried automatically.
Ctrl-C stops a wait; rerun the release later to resume from registry state.
See [crates.io rate limits](https://crates.io/docs/rate-limits).

On partial failure, rerun preflight: already published exact versions are skipped;
yanked versions stop the release. If upload succeeded but tag creation did not,
verify the uploaded source commit and create the missing tag manually. Never infer
which source was uploaded from an unverified local tag or recreate tags blindly.

## After publication

In a fresh directory outside the checkout, install the versioned CLI and run its
shell; create a consumer with exact published dependencies and no workspace patches.
Repeat the application smoke tests against registry-only dependencies. Verify docs.rs
builds, README rendering and supported feature pages. Push the reviewed crate tags,
tag the Isotope snapshot, and publish the matching specification site and changelog.
If a package is defective, assess yanking and publish a corrected new version;
publication is not rolled back by deleting a Git tag.
