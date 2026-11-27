// A Web Worker running tile workers: its own instance of the page's module,
// with its own memory. worker.ts and worker-webgl.ts start it, each on its
// build of the module. The page gives it the module and a control port to
// the map's Web Worker, and the instance starts at once. The map's runtime
// then connects each of its render sessions over the control port, with a
// port of the session's own, which the instance serves (mln_runtime::web's
// tile_worker): requests are the session's bytes, and the replies go back in
// batches, each its bytes and its payloads (pixels, vertices, indices), all
// transferred. The worker reports each batch's size over the control port,
// which the map's Web Worker counts.
//
// A panic aborts the instance (wasm32-unknown-unknown can't unwind), and
// nothing may call into it after that. The worker then sends {fatal:
// message} over the control port and the sessions' ports instead of
// replies, with the message the panic hook left in `mlnPanic` (src/lib.rs),
// and the map stops. The page's message may carry `panic`, a message to
// panic with once the instance starts (`panic_for_check`), for
// check/check.mjs.

import type {
  Bindings,
  FromTileWorker,
  StartTileWorker,
  TileWorkerConnect,
  TileWorkerReport,
} from "./protocol.ts";

/** Runs tile workers in this Web Worker, on a build of the module. */
export function serve({
  default: init,
  mlnWebServeTileWorker,
  panic_for_check,
}: Bindings<"mlnWebServeTileWorker" | "panic_for_check">) {
  let control: MessagePort | null = null;
  const ports: MessagePort[] = [];
  // Once the instance panicked, the panic's message.
  let dead: string | null = null;

  const report = (message: TileWorkerReport) => control?.postMessage(message);
  const tell = (port: MessagePort, message: string) =>
    port.postMessage({ fatal: message } satisfies FromTileWorker);

  function fatal(error: unknown) {
    if (dead !== null) return;
    dead = self.mlnPanic ?? String(error);
    report({ fatal: dead });
    for (const port of ports) tell(port, dead);
  }

  function call(f: () => void) {
    if (dead !== null) return;
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

  // A session's port, whose batches of replies the control port counts.
  function connect(port: MessagePort) {
    if (dead !== null) {
      tell(port, dead);
      return;
    }
    ports.push(port);
    const post = port.postMessage.bind(port);
    port.postMessage = ((message: unknown, transfer: Transferable[]) => {
      if (Array.isArray(message)) report({ replies: message.length });
      post(message, transfer);
    }) as typeof port.postMessage;
    call(() => mlnWebServeTileWorker(port));
  }

  onmessage = async ({ data }: MessageEvent<StartTileWorker>) => {
    control = data.control;
    await init({ module_or_path: data.module });
    if (data.panic) {
      const panic = data.panic;
      call(() => panic_for_check(panic));
    }
    // The control port keeps the connections that arrive before the
    // instance starts.
    data.control.onmessage = ({ data }: MessageEvent<TileWorkerConnect>) =>
      connect(data.port);
  };
}
