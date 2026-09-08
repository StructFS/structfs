# The Isotope site

The website for the Isotope specification, following the same
Eleventy + Cloudflare Pages pattern as `site/` (structfs.com),
`namecode/site/`, and `featherweight/site/`. This site is the spec's:
the model, the rendered chapters, the ABI, and the implementations
catalog. The featherweight project (quickstart, live demo, SDKs) has
its own site at `featherweight/site/`.

## Build

```bash
./build.sh     # copies the spec chapters in, runs 11ty
./dev.sh       # build, then serve with live reload
./clean.sh
```

Needs: Node 20+, pnpm. (No Rust — the spec site has no wasm to build.)

`src/spec/*.md` is **copied in by build.sh** from `../spec` and
gitignored — the site can never drift from the spec. Edit the spec,
never the copies. Note: the dev server watches the copies, so spec
edits need a `./build.sh` re-run to appear.

## Deploying

**The domain is TBD** — all site domains are single-sourced from
`/sites.json` at the repo root; edit the `isotope` entry there when the
real domain is chosen and every cross-link (on this site and its
siblings) plus this site's canonical URL update together.

`.github/workflows/deploy-isotope-site.yml` is `workflow_dispatch`-only
until the Cloudflare side exists. To go live:

1. Create a Cloudflare Pages project named `isotope-site` (or edit the
   `--project-name` in the workflow).
2. Ensure the `structfs_com` environment secrets
   (`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`) cover it, or give
   the workflow its own environment.
3. Point the chosen domain at the Pages project and update the
   `isotope` entry in `/sites.json`.
4. Add the push trigger to the workflow (see the comment in it).
