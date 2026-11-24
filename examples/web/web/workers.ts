// The tile workers' Web Workers (tile-worker.ts), which the page starts for
// the map's Web Worker (main.ts) and for the style picker's previews'
// (picker.ts).

import type { Backend, StartTileWorker } from "./protocol.ts";

/**
 * A tile workers' Web Worker on `backend`'s build of the module. It loads its
 * script at once, and waits for `startTileWorker`.
 */
export function tileWorker(backend: Backend): Worker {
  return backend === "webgl2"
    ? new Worker(new URL("./worker-webgl.ts", import.meta.url), {
        type: "module",
      })
    : new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
}

/**
 * Starts `worker` on `module`. Returns its control port, for the Web Worker
 * whose maps it serves. `panic` is a message to panic with once its instance
 * starts, for check/check.mjs.
 */
export function startTileWorker(
  worker: Worker,
  module: WebAssembly.Module,
  panic?: string,
): MessagePort {
  const channel = new MessageChannel();
  const start: StartTileWorker = { module, control: channel.port2, panic };
  worker.postMessage(start, [channel.port2]);
  return channel.port1;
}
