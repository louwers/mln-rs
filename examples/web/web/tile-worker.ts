// A Web Worker running tile workers: its own instance of the page's module,
// with its own memory. worker.ts and worker-webgl.ts start it, each on its
// build of the module. The instance serves the map's Web Worker over the
// MessageChannel port the page gave it (mln_web_host's tile_worker): requests
// are the map's bytes, and the replies go back in batches, each its bytes and
// its payloads (pixels, vertices, indices), all transferred.
//
// A panic aborts the instance (wasm32-unknown-unknown can't unwind), and
// nothing may call into it after that. The worker sends {fatal: message}
// over the port instead of replies, with the message the panic hook left in
// `mlnPanic` (src/lib.rs), and the map stops. The page's first message may
// carry `panic`, a message to panic with once the instance starts
// (`panic_for_check`), for check/check.mjs.

import type { Bindings, FromTileWorker, StartTileWorker } from "./protocol.ts";

/** Runs tile workers in this Web Worker, on a build of the module. */
export function serve({
  default: init,
  mlnWebServeTileWorker,
  panic_for_check,
}: Bindings<"mlnWebServeTileWorker" | "panic_for_check">) {
  let port: MessagePort | null = null;
  let dead = false;

  function fatal(error: unknown) {
    if (dead) return;
    dead = true;
    const message: FromTileWorker = { fatal: self.mlnPanic ?? String(error) };
    port?.postMessage(message);
  }

  function call(f: () => void) {
    if (dead) return;
    try {
      f();
    } catch (error) {
      fatal(error);
    }
  }

  // What throws outside `call`: the instance's work between requests, a
  // reply's image decode, the instance's start.
  addEventListener("error", (event) => fatal(event.error ?? event.message));
  addEventListener("unhandledrejection", (event) => fatal(event.reason));

  // The port keeps the requests that arrive before the instance serves it.
  onmessage = async ({ data }: MessageEvent<StartTileWorker>) => {
    port = data.port;
    await init({ module_or_path: data.module });
    if (data.panic) {
      const panic = data.panic;
      call(() => panic_for_check(panic));
    }
    const served = data.port;
    call(() => mlnWebServeTileWorker(served));
  };
}
