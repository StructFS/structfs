# Coordinated release procedure

StructFS and Featherweight currently target **0.4.0**, paired with the Isotope
2026-09-14 specification candidate. Namecode is independently versioned. Never
overwrite an existing publication. [Registry status](release-status.md) records
exact-version availability; repeat verification immediately before publication.

The 0.3 packages were verified as published on 2026-09-14. Historical validation
records describe their dated workloads and do not certify this revision. See
[0.4 implementation decisions](design/2026-09-14-coherent-contracts.md) and
[migration](migration-0.4.md) for the new contracts.

## Candidate verification

For the 0.3 release, acceptance is local: the complete release dry-run, rebuilt
browser-host tests, and review of the resulting artifacts. Remote CI status is not
a prerequisite for this release. This does not claim validation on other hosts.

Use Rust 1.96+, Python 3.12 (archive extraction), Node 25.9.0 and npm:

```sh
rustup target add wasm32-unknown-unknown
scripts/prepare-release.sh
scripts/release.sh --plan
scripts/release.sh --dry-run
```

The plan is read-only and offline. The gate runs release-driver regressions,
formatting (including external consumers), all-feature workspace tests and Clippy,
strict workspace docs, isolated facade feature checks, and actual extracted-archive
consumer verification. The archive gate checks guest feature modes, portable profiles, the independent
HTTP/handles browser consumer, detached regressions, release-mode path validation,
and the real loopback disconnect lifecycle example. It never publishes or tags. On macOS the HTTP tests need ordinary
SystemConfiguration access, so a restricted execution sandbox may prevent them
from initializing.

`prepare-release.sh` fetches locked dependencies for the workspace and every
standalone consumer, then installs the browser host's locked npm dependencies.
The standalone lockfiles can resolve versions absent from the workspace cache.
`check-release.sh` remains offline for Cargo checks and rebuilds `kv.wasm` before
running the browser host's TypeScript and Node tests. An existing ignored Wasm
file is never sufficient evidence of a passing release.

For the optional documentation-site artifact, run `CI=true site/build.sh` locally
with wasm-pack and pnpm installed. Here `CI=true` makes dependency installation
noninteractive; the command builds locally and does not deploy anything. The site
uses frozen npm dependency resolution and a locked Cargo build.

The existing CI workflows remain additional automation. Windows and actual browser
engines are not certified by local macOS and Node tests.

Review changelog, migration guide, spec matrix, packaged README links and ignored
tests. The three ignored runtime tests regenerate fixtures or print scheduling
diagnostics; ignored documentation snippets are illustrative and are not executed
acceptance tests. The 10,000-session capacity harness is an explicit additional
performance run, not a substitute for latency/throughput measurements of a real
production provider. Record hardware, toolchain, workload, latency and memory
units with any published performance claim.

The five independent consumers are the repository’s acceptance fixtures and run
against extracted archives. Ox/Horns migration tests require their own environments;
application-specific adoption is separate from this local crate release acceptance.

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

After the candidate passes the local gates and review, commit it on main with a clean tree.
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
builds, README rendering and supported feature pages. Update the changelog and
checkout release-status notices using confirmed registry results; packaged READMEs
describe the version’s API without embedding an unpublished-status claim. Push the reviewed crate tags,
tag the Isotope snapshot, and publish the matching specification site and changelog.
If a package is defective, assess yanking and publish a corrected new version;
publication is not rolled back by deleting a Git tag.

## Registry status record

Run `python3 scripts/release.py --status` to verify selected package versions and
regenerate `docs/release-status.json` and its Markdown projection. It never
publishes. The publication command refreshes this record after successful
publication; if a publication attempt stops partway, run `--status` to record the
partial state before retrying. Missing versions, published versions and yanked
versions are distinct. Link current readiness documentation to this record rather
than inferring publication from local tags or old changelog wording.
