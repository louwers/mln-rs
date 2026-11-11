// A Web Worker running tile workers: its own instance of the page's module,
// with its own memory. It talks to the map's Web Worker over the
// MessageChannel port the page gave it: requests are the map's bytes, and
// the replies go back in batches, each its bytes and its payloads (pixels,
// vertices, indices), all transferred. A raster tile's reply comes once the
// browser has decoded its image, outside a run: the instance asks for one.
//
// A panic aborts the instance (wasm32-unknown-unknown can't unwind), and
// nothing may call into it after that. The worker sends {fatal: message}
// over the port instead of replies, with the message the panic hook left in
// `mlnPanic` (src/lib.rs), and the map stops. The page's first message may
// carry `panic`, a message to panic with once the instance starts
// (`panic_for_check`), for check/check.mjs.

import init, { TileWorker, panic_for_check } from './pkg/mln_web.js';
import type { FromTileWorker, StartTileWorker } from './protocol.ts';

let worker: TileWorker | null = null;
let port: MessagePort | null = null;
const early: Uint8Array[] = []; // requests that arrived before the instance was ready
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

// What throws outside `call`: a reply's image decode, the instance's start.
addEventListener('error', (event) => fatal(event.error ?? event.message));
addEventListener('unhandledrejection', (event) => fatal(event.reason));

// The workers' tasks run in slices of up to 10 ms, so that requests (a
// tile's removal, newer data) get in between them.
const channel = new MessageChannel();
let scheduled = false;
function schedule() {
  if (!scheduled) {
    scheduled = true;
    channel.port2.postMessage(null);
  }
}
channel.port1.onmessage = () => {
  scheduled = false;
  call(() => {
    if (!worker || !port) return;
    const replies: Uint8Array[][] = worker.run(10);
    if (replies.length > 0) port.postMessage(replies, replies.flatMap((parts) => parts.map((part) => part.buffer)));
    if (worker.has_tasks()) schedule();
  });
};

onmessage = async ({ data }: MessageEvent<StartTileWorker>) => {
  port = data.port;
  port.onmessage = ({ data: request }: MessageEvent<Uint8Array>) => {
    if (worker) {
      const running = worker;
      call(() => running.handle(request));
      schedule();
    } else {
      early.push(request);
    }
  };
  await init({ module_or_path: data.module });
  const started = new TileWorker(schedule);
  worker = started;
  if (data.panic) {
    const panic = data.panic;
    call(() => panic_for_check(panic));
  }
  for (const request of early.splice(0)) call(() => started.handle(request));
  schedule();
};
