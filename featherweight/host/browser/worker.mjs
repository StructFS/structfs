// Worker entry: runs a core-binding guest as a resident server.
//
// The guest's synchronous run() lives here, off the main thread. Its
// mailbox read parks in Atomics.wait (ChannelReceiver) until the main
// thread sends a request; responses, stdio, and logs stream back via
// postMessage, which works even while run() holds the worker's event
// loop. A null request is the shutdown signal (spec 07): the guest
// drains, writes iso/shutdown/complete, and run() returns.
//
// Works both as a browser module worker and a Node worker_thread; the
// few lines below bridge the two message APIs.

import { ChannelReceiver } from "./channel.mjs";
import { IsoStore } from "./iso-store.mjs";
import { instantiate } from "./structfs-host.mjs";

const inBrowser = typeof self !== "undefined";
const nodePort = inBrowser
  ? null
  : (await import("node:worker_threads")).parentPort;

const post = (message) =>
  inBrowser ? self.postMessage(message) : nodePort.postMessage(message);

const onMessage = (handler) => {
  if (inBrowser) self.onmessage = (event) => handler(event.data);
  else nodePort.on("message", handler);
};

onMessage(async ({ wasm, sab, args, env }) => {
  try {
    const channel = new ChannelReceiver(sab);
    const iso = new IsoStore({
      args,
      env,
      mailbox: () => {
        const value = channel.receive();
        post({ type: "taken" });
        return value;
      },
      onResponse: (id, value) => post({ type: "response", id, value }),
      onStdio: (stream, text) => post({ type: "stdio", stream, text }),
      onLog: (level, value) => post({ type: "log", level, value }),
    });

    const guest = await instantiate(wasm, iso);
    post({ type: "ready", manifest: guest.manifest() });
    const code = guest.run();
    post({ type: "exit", code, shutdownComplete: iso.shutdownComplete });
  } catch (error) {
    post({ type: "error", message: String(error?.message ?? error) });
  }
});
