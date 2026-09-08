// Static server for the demo, with the COOP/COEP headers that unlock
// SharedArrayBuffer (crossOriginIsolated). Dependency-free.
//
//   npm run build && node serve.ts    then open http://localhost:8787

import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL(".", import.meta.url));
const types: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
};

const handler = async (
  request: IncomingMessage,
  response: ServerResponse,
): Promise<void> => {
  const path = normalize(
    new URL(request.url ?? "/", "http://x").pathname,
  ).replace(/^(\.\.[/\\])+/, "");
  const file = join(root, path === "/" ? "index.html" : path);
  try {
    const body = await readFile(file);
    response.writeHead(200, {
      "Content-Type": types[extname(file)] ?? "application/octet-stream",
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    });
    response.end(body);
  } catch {
    response.writeHead(404).end("not found");
  }
};

const port = Number(process.env["PORT"] ?? 8787);
createServer(handler).listen(port, () => {
  console.log(
    `serving the featherweight browser host demo on http://localhost:${port}`,
  );
});
