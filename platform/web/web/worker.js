// A Web Worker running tile workers: its own instance of the page's module,
// with its own memory. It talks to the map's Web Worker over the
// MessageChannel port the page gave it: requests are the map's bytes, and
// the replies go back in batches, each its bytes and its payloads (pixels,
// vertices, indices), all transferred.

import init, { TileWorker } from './pkg/mln_web.js';

let worker = null;
let port = null;
const early = []; // requests that arrived before the instance was ready
// A panic aborts the instance (wasm32-unknown-unknown can't unwind), and
// nothing may call into it after that: its tiles stay pending.
let dead = false;

function call(f) {
  if (dead) return;
  try {
    f();
  } catch (error) {
    dead = true;
    console.error('tile worker instance dead, its tiles stay pending:', error);
  }
}

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
    const replies = worker.run(10);
    if (replies.length > 0) port.postMessage(replies, replies.flatMap((parts) => parts.map((part) => part.buffer)));
    if (worker.has_tasks()) schedule();
  });
};

onmessage = async ({ data }) => {
  port = data.port;
  port.onmessage = (event) => {
    if (worker) {
      call(() => worker.handle(event.data));
      schedule();
    } else {
      early.push(event.data);
    }
  };
  await init({ module_or_path: data.module });
  worker = new TileWorker();
  for (const request of early.splice(0)) call(() => worker.handle(request));
  schedule();
};
