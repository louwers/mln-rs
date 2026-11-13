// Drives the browser package's raw C ABI as a JavaScript host does, in a
// module Web Worker: a runtime, a map with a GeoJSON style, a session on an
// OffscreenCanvas, and frames until the canvas shows the style. Struct
// offsets are wasm32's (clang's layout of the headers). Posts the outcome to
// the page.

const params = new URL(self.location.href).searchParams;
// The page logs what this worker logs, and the check prints it.
const log = (...args) => postMessage({ log: args.map(String) });
const pkg = params.get('pkg');

// Counts the tile workers' Web Workers that the library starts.
let spawned = 0;
const NativeWorker = globalThis.Worker;
globalThis.Worker = class extends NativeWorker {
  constructor(...args) {
    super(...args);
    spawned++;
  }
};

const glue = await import(`${pkg}/maplibre_native_c.js`);
const wasm = await glue.default();

const SIZE = 256;
const view = () => new DataView(wasm.memory.buffer);
function alloc(size, align = 8) {
  const pointer = wasm.mln_web_alloc(size, align);
  new Uint8Array(wasm.memory.buffer, pointer, size).fill(0);
  return pointer;
}
/** A mln_buffer_view of `text`'s UTF-8, by address. */
function text(string) {
  const bytes = new TextEncoder().encode(string);
  const data = alloc(bytes.length, 1);
  new Uint8Array(wasm.memory.buffer, data, bytes.length).set(bytes);
  const pointer = alloc(8, 4);
  view().setUint32(pointer, data, true);
  view().setUint32(pointer + 4, bytes.length, true);
  return pointer;
}
// A mln_diagnostic for each call, whose message a failure reports.
const DIAGNOSTIC_SIZE = 4 + 4096;
const diagnostic = alloc(DIAGNOSTIC_SIZE, 4);
function check(status, what) {
  if (status === 0) return;
  const bytes = new Uint8Array(wasm.memory.buffer, diagnostic + 4, DIAGNOSTIC_SIZE - 4);
  const message = new TextDecoder().decode(bytes.subarray(0, bytes.indexOf(0)));
  throw new Error(`${what} failed with status ${status}: ${message}`);
}
function prepare() {
  view().setUint32(diagnostic, DIAGNOSTIC_SIZE, true);
  return diagnostic;
}

// Settles the check once the canvas shows the layer, or the check fails.
let settled = null;

// Callbacks, by the key in their user data.
const handlers = new Map();
let nextKey = 1;
// A handler that throws fails the check: the library logs and drops it.
glue.mlnWebSetUpcallDispatcher((name, userData, ...args) => {
  try {
    return handlers.get(userData)?.(name, ...args);
  } catch (error) {
    console.error(`${name}: ${error}`);
    settled?.reject(error);
  }
});
function callback(handler) {
  const key = nextKey++;
  handlers.set(key, handler);
  return key;
}
/** A mln_completion that resolves the returned promise with its status. */
function completion() {
  let resolve;
  const done = new Promise((r) => (resolve = r));
  const pointer = alloc(24);
  view().setUint32(pointer, 24, true);
  view().setUint32(pointer + 4, glue.mlnWebUpcallPointer('mln_completion_callback'), true);
  view().setUint32(pointer + 8, callback((_, result) => resolve(view().getUint32(result + 4, true))), true);
  return { pointer, done };
}

let out = alloc(8);
check(wasm.mln_runtime_create(0, out, prepare()), 'mln_runtime_create');
const runtime = view().getBigUint64(out, true);
log('runtime', runtime);

const options = alloc(56);
view().setUint32(options, 56, true);
view().setUint32(options + 8, SIZE, true);
view().setUint32(options + 12, SIZE, true);
view().setFloat64(options + 16, 1, true);
let created = completion();
out = alloc(8);
check(wasm.mln_runtime_create_map(runtime, options, out, created.pointer, prepare()), 'mln_runtime_create_map');
const map = view().getBigUint64(out, true);
check(await created.done, 'the map');
log('map', map);

// Red, with a blue polygon over most of the world that tile workers lay out.
const style = {
  version: 8,
  sources: {
    shape: {
      type: 'geojson',
      data: {
        type: 'Polygon',
        coordinates: [[[-170, -80], [170, -80], [170, 80], [-170, 80], [-170, -80]]],
      },
    },
  },
  layers: [
    { id: 'background', type: 'background', paint: { 'background-color': '#ff0000' } },
    { id: 'shape', type: 'fill', source: 'shape', paint: { 'fill-color': '#0000ff' } },
  ],
};
const styled = completion();
check(wasm.mln_map_set_style_json(map, text(JSON.stringify(style)), styled.pointer, prepare()), 'mln_map_set_style_json');

const canvas = new OffscreenCanvas(SIZE, SIZE);
const target = alloc(64);
view().setUint32(target, 64, true);
view().setUint32(target + 4, 17, true); // MLN_RENDER_TARGET_KIND_WEB_CANVAS
view().setUint32(target + 8, SIZE, true);
view().setUint32(target + 12, SIZE, true);
view().setUint32(target + 16, glue.mlnWebObjectInsert(canvas), true);

let session = 0n;
const demand = alloc(16);
view().setUint32(demand, 16, true);
function demandFrame() {
  check(wasm.mln_render_session_demand_frame(session, demand, prepare()), 'mln_render_session_demand_frame');
}

const scratch = new OffscreenCanvas(SIZE, SIZE).getContext('2d', { willReadFrequently: true });
const deadline = performance.now() + 60000;
let frames = 0;
const outcome = new Promise((resolve, reject) => (settled = { resolve, reject }));
const batch = alloc(8);
const results = alloc(16);
const wake = glue.mlnWebUpcallPointer('mln_wake_callback');
const attach = alloc(96);
view().setUint32(attach, 96, true);
view().setUint32(attach + 4, 1, true); // MLN_RENDER_DRIVER_CORE_WORKER
// The frame wake runs in the task that rendered, while the canvas still has
// the frame.
view().setUint32(attach + 16, wake, true);
view().setUint32(attach + 20, callback(() => {
  view().setBigUint64(batch, 0n, true);
  view().setUint32(results, 16, true);
  check(wasm.mln_render_session_drain_frame_results(session, batch, results, prepare()), 'drain');
  const count = view().getUint32(results + 12, true);
  const stride = view().getUint32(results + 4, true);
  const first = view().getUint32(results + 8, true);
  let rendered = false;
  for (let i = 0; i < count; i++) {
    const disposition = view().getUint32(first + i * stride + 4, true);
    rendered ||= disposition === 0;
  }
  wasm.mln_render_frame_batch_release(view().getBigUint64(batch, true));
  if (!rendered) return;
  frames++;
  scratch.drawImage(canvas, 0, 0);
  const [r, g, b] = scratch.getImageData(SIZE / 2, SIZE / 2, 1, 1).data;
  if (b > 200 && r < 50) settled.resolve({ frames, spawned, pixel: [r, g, b] });
  else if (performance.now() > deadline) settled.reject(new Error(`the center is still ${[r, g, b]} after ${frames} frames`));
}), true);
view().setUint32(attach + 40, wake, true);
view().setUint32(attach + 44, callback(() => {
  demandFrame();
}), true);
const attached = completion();
out = alloc(8);
check(wasm.mln_map_attach(map, target, 0n, attach, out, attached.pointer, prepare()), 'mln_map_attach');
session = view().getBigUint64(out, true);
log('attaching', session);
check(await attached.done, 'the attach');
log('attached');
check(await styled.done, 'the style');
log('styled');
demandFrame();

try {
  postMessage({ ok: true, backend: params.get('backend'), ...(await outcome) });
} catch (error) {
  postMessage({ ok: false, error: String(error), frames, spawned });
}
