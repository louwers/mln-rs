// Drives the browser package's raw C ABI as a JavaScript host does, in a
// module Web Worker: a runtime, a map with a GeoJSON style, and sessions one
// after another, each on a new OffscreenCanvas and a device of its own, that
// render until the canvas shows the style. The sessions run on a core worker,
// or with `?driver=caller` on this thread's services of their driver work. A
// session that fails reports its failure. Struct
// offsets are wasm32's (clang's layout of the headers). Posts the outcome to
// the page.

const params = new URL(self.location.href).searchParams;
// The page logs what this worker logs, and the check prints it.
const log = (...args) => postMessage({ log: args.map(String) });
const pkg = params.get("pkg");
const driver = params.get("driver") ?? "core";

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
// A panic logs its message before it aborts the instance, which fails the
// check.
glue.mlnWebSetPanicHook();

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
  const bytes = new Uint8Array(
    wasm.memory.buffer,
    diagnostic + 4,
    DIAGNOSTIC_SIZE - 4,
  );
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
  view().setUint32(
    pointer + 4,
    glue.mlnWebUpcallPointer("mln_completion_callback"),
    true,
  );
  view().setUint32(
    pointer + 8,
    callback((_, result) => resolve(view().getUint32(result + 4, true))),
    true,
  );
  return { pointer, done };
}

let out = alloc(8);
check(wasm.mln_runtime_create(0, out, prepare()), "mln_runtime_create");
const runtime = view().getBigUint64(out, true);
log("runtime", runtime);

const options = alloc(56);
view().setUint32(options, 56, true);
view().setUint32(options + 8, SIZE, true);
view().setUint32(options + 12, SIZE, true);
view().setFloat64(options + 16, 1, true);
let created = completion();
out = alloc(8);
check(
  wasm.mln_runtime_create_map(
    runtime,
    options,
    out,
    created.pointer,
    prepare(),
  ),
  "mln_runtime_create_map",
);
const map = view().getBigUint64(out, true);
check(await created.done, "the map");
log("map", map);

// Red, with a blue polygon over most of the world that tile workers lay out,
// and a white CJK label below the center, which the browser draws in its
// sans-serif font: the map has system fonts by default, and no glyph PBF is
// fetched for it. The label needs a CJK font on the system.
const style = {
  version: 8,
  glyphs: "https://glyphs.invalid/{fontstack}/{range}.pbf",
  sources: {
    label: {
      type: "geojson",
      data: { type: "Point", coordinates: [0, -40] },
    },
    shape: {
      type: "geojson",
      data: {
        type: "Polygon",
        coordinates: [
          [
            [-170, -80],
            [170, -80],
            [170, 80],
            [-170, 80],
            [-170, -80],
          ],
        ],
      },
    },
  },
  layers: [
    {
      id: "background",
      type: "background",
      paint: { "background-color": "#ff0000" },
    },
    {
      id: "shape",
      type: "fill",
      source: "shape",
      paint: { "fill-color": "#0000ff" },
    },
    {
      id: "label",
      type: "symbol",
      source: "label",
      layout: {
        "text-field": "中文地图",
        "text-font": ["Noto Sans Regular"],
        "text-size": 24,
      },
      paint: { "text-color": "#ffffff" },
    },
  ],
};

/** Whether the scratch canvas shows the label: white pixels. */
function showsLabel() {
  const pixels = scratch.getImageData(0, 0, SIZE, SIZE).data;
  let white = 0;
  for (let i = 0; i < pixels.length; i += 4)
    if (pixels[i] > 200 && pixels[i + 1] > 200 && pixels[i + 2] > 200) white++;
  return white > 20;
}
const styled = completion();
check(
  wasm.mln_map_set_style_json(
    map,
    text(JSON.stringify(style)),
    styled.pointer,
    prepare(),
  ),
  "mln_map_set_style_json",
);

const demand = alloc(16);
view().setUint32(demand, 16, true);
const scratch = new OffscreenCanvas(SIZE, SIZE).getContext("2d", {
  willReadFrequently: true,
});
const batch = alloc(8);
const results = alloc(16);
const abandoned = alloc(16);
const failure = alloc(1024, 1);
const failureLength = alloc(8);
const wake = glue.mlnWebUpcallPointer("mln_wake_callback");
let frames = 0;

/** The message of a failed session. */
function failureOf(session) {
  check(
    wasm.mln_render_session_get_failure(
      session,
      failure,
      1024,
      failureLength,
      prepare(),
    ),
    "mln_render_session_get_failure",
  );
  const length = Math.min(view().getUint32(failureLength, true), 1023);
  return new TextDecoder().decode(
    new Uint8Array(wasm.memory.buffer, failure, length),
  );
}

/**
 * Attaches a session on a device of its own to a new OffscreenCanvas,
 * renders until the canvas shows the layer, and ends the session by `end`:
 * "detach" detaches and then releases it, and "abandon" abandons and then
 * releases it, as a binding's finalizer does.
 */
async function show(label, end) {
  const canvas = new OffscreenCanvas(SIZE, SIZE);
  const handle = glue.mlnWebObjectInsert(canvas);
  const target = alloc(64);
  view().setUint32(target, 64, true);
  view().setUint32(target + 4, 17, true); // MLN_RENDER_TARGET_KIND_WEB_CANVAS
  view().setUint32(target + 8, SIZE, true);
  view().setUint32(target + 12, SIZE, true);
  view().setUint32(target + 16, handle, true);

  let session = 0n;
  let released = false;
  const demandFrame = () => {
    if (!released)
      check(
        wasm.mln_render_session_demand_frame(session, demand, prepare()),
        `${label}: demand`,
      );
  };
  // What the last frame showed. A map that is done drawing renders no more
  // frames, so a timer, not a frame, fails the session.
  let seen = "no frame";
  const outcome = new Promise(
    (resolve, reject) => (settled = { resolve, reject }),
  );
  const timer = setTimeout(
    () => settled.reject(new Error(`${label}: after 60 s, ${seen}`)),
    60000,
  );
  outcome.finally(() => clearTimeout(timer)).catch(() => {});
  const attach = alloc(96);
  view().setUint32(attach, 96, true);
  // MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD or MLN_RENDER_DRIVER_CORE_WORKER.
  view().setUint32(attach + 4, driver === "caller" ? 2 : 1, true);
  // The frame wake runs in the task that rendered, while the canvas still has
  // the frame.
  view().setUint32(attach + 16, wake, true);
  view().setUint32(
    attach + 20,
    callback(() => {
      if (released) return;
      view().setBigUint64(batch, 0n, true);
      view().setUint32(results, 16, true);
      check(
        wasm.mln_render_session_drain_frame_results(
          session,
          batch,
          results,
          prepare(),
        ),
        `${label}: drain`,
      );
      const count = view().getUint32(results + 12, true);
      const stride = view().getUint32(results + 4, true);
      const first = view().getUint32(results + 8, true);
      let rendered = false;
      let failed = false;
      for (let i = 0; i < count; i++) {
        const disposition = view().getUint32(first + i * stride + 4, true);
        rendered ||= disposition === 0; // MLN_RENDER_RESULT_RENDERED
        failed ||= disposition === 4; // MLN_RENDER_RESULT_FAILED
      }
      wasm.mln_render_frame_batch_release(view().getBigUint64(batch, true));
      if (failed)
        settled.reject(new Error(`${label} failed: ${failureOf(session)}`));
      if (!rendered) return;
      frames++;
      scratch.clearRect(0, 0, SIZE, SIZE);
      scratch.drawImage(canvas, 0, 0);
      const [r, g, b] = scratch.getImageData(SIZE / 2, SIZE / 2, 1, 1).data;
      const labelled = showsLabel();
      if (b > 200 && r < 50 && labelled) settled.resolve();
      seen = `the center ${[r, g, b]}, ${labelled ? "with" : "without"} the label`;
    }),
    true,
  );
  view().setUint32(attach + 40, wake, true);
  view().setUint32(attach + 44, callback(demandFrame), true);
  if (driver === "caller") {
    // The driver-work wake can run inside a library call, so the service runs
    // in a microtask of its own.
    let queued = false;
    view().setUint32(attach + 64, wake, true);
    view().setUint32(
      attach + 68,
      callback(() => {
        if (queued) return;
        queued = true;
        queueMicrotask(() => {
          queued = false;
          if (!released)
            check(
              wasm.mln_render_session_service_driver_work(session, prepare()),
              `${label}: service`,
            );
        });
      }),
      true,
    );
  }
  const attached = completion();
  out = alloc(8);
  check(
    wasm.mln_map_attach(
      map,
      target,
      0n,
      attach,
      out,
      attached.pointer,
      prepare(),
    ),
    `${label}: mln_map_attach`,
  );
  session = view().getBigUint64(out, true);
  check(await attached.done, `${label}: the attach`);
  log(label, "attached");
  check(await styled.done, "the style");
  demandFrame();
  await outcome;
  log(label, "shown; ending it with", end);

  if (end === "detach") {
    const detached = completion();
    check(
      wasm.mln_render_session_detach(session, detached.pointer, prepare()),
      `${label}: mln_render_session_detach`,
    );
    check(await detached.done, `${label}: the detach`);
  } else {
    view().setUint32(abandoned, 16, true);
    check(
      wasm.mln_render_session_abandon(session, abandoned, prepare()),
      `${label}: mln_render_session_abandon`,
    );
  }
  const release = completion();
  check(
    wasm.mln_render_session_release(session, release.pointer, prepare()),
    `${label}: mln_render_session_release`,
  );
  check(await release.done, `${label}: the release`);
  released = true;
  wasm.mln_web_object_release(handle);
}

// Each session asks the browser for a device of its own, which must render
// after the sessions before it ended either way.
try {
  await show("the first session", "detach");
  await show("the second session", "abandon");
  await show("the third session", "detach");
  postMessage({
    ok: true,
    backend: params.get("backend"),
    sessions: 3,
    frames,
    spawned,
  });
} catch (error) {
  postMessage({ ok: false, error: String(error), frames, spawned });
}
