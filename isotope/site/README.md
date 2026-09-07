# The Isotope site

The website for the Isotope spec and the featherweight runtime,
following the same Eleventy + Cloudflare Pages pattern as `site/`
(structfs.com) and `namecode/site/`.

## Build

```bash
./build.sh     # copies spec + browser host in, builds kv.wasm, runs 11ty
./dev.sh       # build, then serve with live reload
./clean.sh
```

Needs: Rust with the `wasm32-unknown-unknown` target, Node 20+, pnpm.

`src/spec/*.md` and `src/demo/` are **copied in by build.sh** (from
`../spec` and `featherweight/host/browser` + the compiled kv guest) and
gitignored — the site can never drift from the spec or the host. Edit
the originals, never the copies.

## The live demo and COOP/COEP

`/demo/` runs kv.wasm resident in a Web Worker, which needs
`SharedArrayBuffer` and therefore cross-origin isolation. `src/_headers`
applies COOP/COEP site-wide on Cloudflare Pages. The Eleventy dev server
sends no such headers, so locally the demo degrades to batch mode (each
request re-instantiates the block); `pnpm dlx wrangler pages dev _site`
serves the production headers if you need the resident mode locally.

Because of the site-wide `Cross-Origin-Embedder-Policy: require-corp`,
any future cross-origin embed (fonts, images, iframes from other
domains) must carry CORP/CORS headers or the browser will block it.

## Deploying

**The domain is TBD** — `src/_data/site.json` currently says
`https://isotope.structfs.com` as a placeholder; update it when the real
domain is chosen.

`.github/workflows/deploy-isotope-site.yml` is `workflow_dispatch`-only
until the Cloudflare side exists. To go live:

1. Create a Cloudflare Pages project named `isotope-site` (or edit the
   `--project-name` in the workflow).
2. Ensure the `structfs_com` environment secrets
   (`CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`) cover it, or give
   the workflow its own environment.
3. Point the chosen domain at the Pages project and update
   `src/_data/site.json`.
4. Add the push trigger to the workflow (see the comment in it).
