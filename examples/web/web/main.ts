// The page: it runs no wasm itself. It compiles the module, gives its canvas
// to the map's Web Worker (map-worker.ts) as an OffscreenCanvas, and starts
// the tile workers' Web Workers (worker.ts) while the module compiles, each
// with a control port to the map's, where the map's runtime connects them
// with MessageChannels of their own, so that their requests and replies
// never pass through this thread. Each Web Worker instantiates the
// same module with its own memory, so nothing needs SharedArrayBuffer or
// COOP/COEP headers.
//
// The map draws with WebGPU. A browser without it gets the module's other
// build, which draws with WebGL2 (pkg/webgl, worker-webgl.ts). So does one
// whose WebGPU can't start the map: the page then loads again with WebGL2,
// and the tab remembers that. The control at the bottom left shows the
// backend and, where the other one can draw, loads the page again with it.
//
// URL parameters:
//   style=<key|url>        a style of styles.ts, or any style's URL
//                          (default: OpenFreeMap Liberty)
//   fallbackFontFaces=<json>
//                          font files that every font stack falls back to, as
//                          in a style's font-faces entry without a font name:
//                          [{"url": "...", "unicode-range": ["U+1000-109F"]}]
//   lat, lon, zoom, bearing, pitch
//                          the camera, unless the hash has one
//   workers=<n>            Web Workers for the tiles (default: up to 4)
//   width, height, ratio   a fixed map size in CSS pixels and pixel ratio
//                          (default: the window and devicePixelRatio)
//   localIdeographFontFamily=false
//                          download CJK glyphs rather than draw them with
//                          the browser's fonts, as GL JS's option does when
//                          false (or empty)
//   compare                MapLibre GL JS's map of the style beside this one
//                          (compare.ts)
//   projection=<flat|globe|handoff>
//                          the projection the map draws with in place of the
//                          style's: Web Mercator, the sphere at every zoom
//                          (vertical-perspective), or the sphere handing over
//                          to Web Mercator between zoom 11 and 12 (globe)
//                          (default: the style's own)
//   freeBearing            free bearing on the globe: drags and zooms on it
//                          turn the bearing with the planet, and a drag
//                          carries on over a pole (default: off, north
//                          stays up)
//   debug                  the debug overlay (debug.ts), which D also
//                          toggles
//   panic=<map|tile>       the map's instance or the first tile worker's
//                          panics once it starts, for check/check.mjs
//   backend=<webgpu|webgl2>
//                          what the map draws with (default: WebGPU where
//                          the browser has it)
//
// The hash holds the camera as GL JS's does, #zoom/lat/lon[/bearing/pitch],
// and follows it, so that a link opens the same view. Picking a style keeps
// the camera and puts the style's key in `?style=`.
//
// Input: the canvas's pointer, wheel, key and focus events go to the map's
// Web Worker, whose gesture recognizer (mln_gestures) moves the camera: drag
// to pan, right- or Ctrl-drag to rotate and tilt, Shift-drag to zoom to a
// box, the wheel or a trackpad to zoom, double-click or double-tap to zoom
// in (Shift: out), two fingers to pinch, twist and tilt, and the arrow,
// +/= and - keys. The map follows its box's size (ResizeObserver). The zoom
// buttons send the + and - keys; the compass eases the bearing and pitch
// back to zero.
//
// `window.mln` is for scripts (check/check.mjs): `backend` is what the map
// draws with, `loaded` resolves with the
// load time and the number of worker replies once the map is fully loaded,
// and `readPixels()` draws the frame again and resolves with its RGBA
// pixels, read back from the canvas. `fetches()` resolves with every URL
// the map's Web Worker fetched and how many times, `idle()` with the camera
// once the map is fully loaded and has stopped moving, and `nextFrame()`
// once the map has run its next animation frame. `checkGpu()` runs a check
// of a GPU of the map's backend (WebMap::start_gpu_check) and resolves once
// it passed. A panic in any of the Web Workers' wasm instances stops the
// map: the page shows its message, `fatal` resolves with it as an Error,
// and `loaded` and the methods reject with it.

import type { Comparison } from "./compare.ts";
import { createDebugOverlay } from "./debug.ts";
import wasmUrl from "./pkg/mln_web_bg.wasm?url";
import webglWasmUrl from "./pkg/webgl/mln_web_bg.wasm?url";
import {
  type Backend,
  type Box,
  type Camera,
  type FromMap,
  type Input,
  type Loaded,
  type Pixels,
  type ProjectionType,
  type StartTileWorker,
  type ToMap,
} from "./protocol.ts";
import { STYLES, type Style, sameStyle, styleFor } from "./styles.ts";
import { wheelDelta } from "./wheel.ts";

declare global {
  interface Window {
    mln: {
      backend: Backend;
      loaded: Promise<Loaded>;
      fatal: Promise<Error>;
      readPixels(): Promise<Pixels>;
      fetches(): Promise<Record<string, number>>;
      idle(): Promise<Camera>;
      nextFrame(): Promise<void>;
      checkGpu(): Promise<void>;
    };
  }
}

/** The element with `id`, which must be a `type`. */
function element<T extends Element>(
  id: string,
  type: abstract new (...args: never[]) => T,
): T {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`#${id} is not a ${type.name}`);
  return found;
}

const params = new URLSearchParams(location.search);
const number = (name: string, fallback: number) =>
  params.has(name) ? Number(params.get(name)) : fallback;
const status = element("status", HTMLElement);
const banner = element("error", HTMLElement);
const mapBox = element("map-box", HTMLElement);
const canvas = element("map", HTMLCanvasElement);
// The debug overlay (debug.ts), wired up at the end.
const debug = createDebugOverlay(element("bottom-left", HTMLElement));

if (params.has("width") && params.has("height")) {
  mapBox.style.width = `${number("width", 0)}px`;
  mapBox.style.height = `${number("height", 0)}px`;
}
let ratio = number("ratio", window.devicePixelRatio || 1);
// The map's size in CSS pixels, its box's; the worker resizes the map and
// its surface. Its first report is the size the map started at.
let size: [number, number] = [mapBox.clientWidth, mapBox.clientHeight];
// The map draws its size times the pixel ratio, rounded down, in device
// pixels (mln_runtime's render session). The canvas's box covers exactly
// those, as GL JS's does: at a fractional pixel ratio, a box of whole CSS
// pixels covers a fractional number of device pixels, and a browser that
// resamples the canvas into it blurs the map.
const devicePixels = (length: number) => Math.floor(length * ratio);
function fitCanvas() {
  canvas.style.width = `${devicePixels(size[0]) / ratio}px`;
  canvas.style.height = `${devicePixels(size[1]) / ratio}px`;
}
canvas.width = devicePixels(size[0]);
canvas.height = devicePixels(size[1]);
fitCanvas();

const localFamily = params.get("localIdeographFontFamily");

const BACKEND_NAME: Record<Backend, string> = {
  webgpu: "WebGPU",
  webgl2: "WebGL2",
};
const OTHER_BACKEND: Record<Backend, Backend> = {
  webgpu: "webgl2",
  webgl2: "webgpu",
};

// The backends that couldn't start the map in this tab: the page neither
// starts with nor offers them again.
const FAILED_KEY = "mln-failed-backends";
function failedBackends(): Backend[] {
  try {
    return (sessionStorage.getItem(FAILED_KEY)?.split(",") ?? []).filter(
      (name): name is Backend => name in BACKEND_NAME,
    );
  } catch {
    return []; // no storage for this page
  }
}
const failed = failedBackends();
function hasApi(backend: Backend): boolean {
  return backend === "webgpu"
    ? "gpu" in navigator
    : "WebGL2RenderingContext" in window;
}
function canDraw(backend: Backend): boolean {
  return hasApi(backend) && !failed.includes(backend);
}
const requested = params.get("backend");
const backend: Backend =
  requested === "webgpu" || requested === "webgl2"
    ? requested
    : canDraw("webgpu")
      ? "webgpu"
      : "webgl2";

const compiling = WebAssembly.compileStreaming(
  fetch(backend === "webgl2" ? webglWasmUrl : wasmUrl),
);

// The Web Workers load their scripts while the module downloads and
// compiles, and get it once it has.
const count = number(
  "workers",
  Math.max(1, Math.min(4, (navigator.hardwareConcurrency || 2) - 1)),
);
const tileWorkers = Array.from({ length: count }, () =>
  backend === "webgl2"
    ? new Worker(new URL("./worker-webgl.ts", import.meta.url), {
        type: "module",
      })
    : new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }),
);
const map =
  backend === "webgl2"
    ? new Worker(new URL("./map-worker.ts", import.meta.url), {
        type: "module",
        name: "webgl2",
      })
    : new Worker(new URL("./map-worker.ts", import.meta.url), {
        type: "module",
      });
// The map's Web Worker may draw between any two of its messages. A touch's
// fingers move with an event each: sent one by one, a pinch would draw with
// one finger moved and not yet the other, zoomed far in or out for a frame,
// and the map would load that zoom's tiles. So the page's inputs (`send`)
// go together at the end of the task they came in, and before any other
// message, which keeps their order.
let inputs: Input[] = [];
function sendInputs() {
  if (inputs.length === 0) return;
  map.postMessage({ type: "input", inputs } satisfies ToMap);
  inputs = [];
}
const endOfTask = new MessageChannel();
endOfTask.port1.onmessage = sendInputs;
const toMap = (message: ToMap, transfer: Transferable[] = []) => {
  sendInputs();
  map.postMessage(message, transfer);
};
const camera: Camera = cameraFromHash(location.hash) ?? [
  number("lat", 52.37),
  number("lon", 4.89),
  number("zoom", 13),
  number("bearing", 0),
  number("pitch", 0),
];
let current = styleFor(params.get("style"));
const { source: style, terrain } = current;
// The map's Web Worker fetches the style and what it needs first meanwhile.
toMap({ type: "preload", style, ratio, zoom: camera[2] });

const panic = params.get("panic");
const panicMessage = "panic=<map|tile> asked for this panic";

// The rest of the page starts meanwhile: its controls work at once, and
// what they ask of the map waits in the map's Web Worker until it starts.
compiling.then(
  (module) => {
    const controls = tileWorkers.map((worker, i) => {
      const channel = new MessageChannel();
      const start: StartTileWorker = {
        module,
        control: channel.port2,
        panic: panic === "tile" && i === 0 ? panicMessage : undefined,
      };
      worker.postMessage(start, [channel.port2]);
      return channel.port1;
    });

    const offscreen = canvas.transferControlToOffscreen();
    toMap(
      {
        type: "start",
        module,
        canvas: offscreen,
        tileWorkers: controls,
        width: size[0],
        height: size[1],
        ratio,
        style,
        terrain,
        fallbackFontFaces: params.get("fallbackFontFaces"),
        camera,
        localIdeographs: localFamily !== "" && localFamily !== "false",
        panic: panic === "map" ? panicMessage : undefined,
      },
      [offscreen, ...controls],
    );
  },
  (error: unknown) =>
    stop(
      `the module didn't compile: ${error instanceof Error ? error.message : String(error)}`,
    ),
);

function showBanner(text: string) {
  banner.textContent = text;
  banner.hidden = false;
}

// The browser's API couldn't draw the map. The tab remembers that, and the
// page loads again with the other backend unless it can't draw either.
function unsupported(message: string) {
  const other = OTHER_BACKEND[backend];
  if (!canDraw(other)) {
    stop(
      `this browser has neither WebGPU nor WebGL2 for the map to draw with (${message})`,
    );
    return;
  }
  // Without storage the URL carries the choice; with it, the page drops a
  // `?backend=` that would pick the failed backend again.
  const url = new URL(location.href);
  try {
    sessionStorage.setItem(
      FAILED_KEY,
      [...new Set([...failed, backend])].join(","),
    );
    url.searchParams.delete("backend");
  } catch {
    url.searchParams.set("backend", other);
  }
  // Replacing a URL with itself and a hash wouldn't load the page again.
  if (url.href === location.href) location.reload();
  else location.replace(url);
}

// When the map last lost its device in this tab, and how soon after that
// another loss counts as a backend that can't keep its device.
const LOST_KEY = "mln-lost-device";
const LOST_AGAIN_MS = 60_000;

// The map lost its device. The page loads again, at the camera that the
// hash keeps, written first, rather than giving the map a new device: once
// Chromium's GPU process restarts, the worker's canvas shows no new frames,
// and the worker gets no animation frames. A second loss within a minute,
// or one the tab has no storage to remember, counts as a backend that can't
// draw the map, since some browsers lose every new device at its first
// frame.
function lost(message: string) {
  history.replaceState(history.state, "", hashFromCamera(latestCamera));
  try {
    const last = Number(sessionStorage.getItem(LOST_KEY));
    if (Date.now() - last >= LOST_AGAIN_MS) {
      sessionStorage.setItem(LOST_KEY, String(Date.now()));
      location.reload();
      return;
    }
  } catch {
    // no storage for this page
  }
  unsupported(message);
}

let markLoaded: (loaded: Loaded) => void = () => {};
let failLoaded: (error: Error) => void = () => {};
let markFatal: (error: Error) => void = () => {};
let fatal: Error | null = null;
interface Pending<T> {
  resolve: (value: T) => void;
  reject: (error: Error) => void;
}
const pixelRequests: Pending<Pixels>[] = [];
const fetchesRequests: Pending<Record<string, number>>[] = [];
const idleRequests: Pending<Camera>[] = [];
const nextFrameRequests: Pending<void>[] = [];
const gpuCheckRequests: Pending<void>[] = [];
/** Asks the map's Web Worker with `message`; its answer resolves the first of `queue`. */
function ask<T>(queue: Pending<T>[], message: ToMap): Promise<T> {
  return new Promise((resolve, reject) => {
    if (fatal) {
      reject(fatal);
      return;
    }
    queue.push({ resolve, reject });
    toMap(message);
  });
}
window.mln = {
  backend,
  loaded: new Promise((resolve, reject) => {
    markLoaded = resolve;
    failLoaded = reject;
  }),
  fatal: new Promise((resolve) => {
    markFatal = resolve;
  }),
  readPixels: () => ask(pixelRequests, { type: "readPixels" }),
  fetches: () => ask(fetchesRequests, { type: "fetches" }),
  idle: () => ask(idleRequests, { type: "idle" }),
  nextFrame: () => ask(nextFrameRequests, { type: "nextFrame" }),
  checkGpu: () => ask(gpuCheckRequests, { type: "checkGpu" }),
};
// Rejected on a panic, which the page reports itself.
window.mln.loaded.catch(() => {});

map.onmessage = ({ data }: MessageEvent<FromMap>) => {
  switch (data.type) {
    case "box":
      showBox(data.box);
      break;
    case "notch":
      // Phones that vibrate tick; elsewhere `vibrate` is missing or does
      // nothing.
      navigator.vibrate?.(10);
      break;
    case "status":
      status.textContent = data.text;
      break;
    case "loaded":
      markLoaded(data.loaded);
      loading = false;
      showStyle();
      break;
    case "pixels":
      pixelRequests.shift()?.resolve(data.pixels);
      break;
    case "fetches":
      fetchesRequests.shift()?.resolve(data.fetches);
      break;
    case "idle":
      idleRequests.shift()?.resolve(data.camera);
      break;
    case "nextFrame":
      nextFrameRequests.shift()?.resolve();
      break;
    case "gpuChecked":
      for (const request of gpuCheckRequests.splice(0)) request.resolve();
      break;
    case "fatal":
      stop(data.message);
      break;
    case "unsupported":
      unsupported(data.message);
      break;
    case "lost":
      lost(data.message);
      break;
    case "camera":
      showCamera(data.camera);
      break;
    case "attribution":
      showAttribution(data.attribution);
      break;
    case "styleName":
      styleName = data.name;
      showStyle();
      break;
    case "frame":
      debug.frame(data.sample);
      break;
    case "stats":
      debug.stats(data.stats);
      break;
  }
};

function stop(message: string) {
  const error = new Error(message);
  fatal = error;
  console.error(`the map stopped: ${message}`);
  showBanner(`The map stopped: ${message}`);
  status.textContent = "stopped";
  showStyle();
  failLoaded(error);
  markFatal(error);
  for (const queue of [
    pixelRequests,
    fetchesRequests,
    idleRequests,
    nextFrameRequests,
    gpuCheckRequests,
  ]) {
    for (const request of queue.splice(0)) request.reject(error);
  }
}

const send = (input: Input) => {
  if (inputs.push(input) === 1) endOfTask.port2.postMessage(null);
};
// Each event's time, as performance.timeOrigin + timeStamp: the map's Web
// Worker turns it into its own performance.now(), the clock its frames run on.
const time = (e: Event) => performance.timeOrigin + e.timeStamp;
const modifiers = (e: MouseEvent | KeyboardEvent) =>
  (e.shiftKey ? 1 : 0) |
  (e.ctrlKey ? 2 : 0) |
  (e.altKey ? 4 : 0) |
  (e.metaKey ? 8 : 0);
// The keys the map uses; Escape only ends a box zoom, so its default stays.
const KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "+",
  "=",
  "-",
  "_",
]);
// Where the canvas is: read on each press and resize rather than on every
// move, which would lay the page out again.
let origin = canvas.getBoundingClientRect();
const at = (e: MouseEvent) => ({
  x: e.clientX - origin.left,
  y: e.clientY - origin.top,
});

canvas.tabIndex = 0;
canvas.style.touchAction = "none";
canvas.style.outline = "none";
canvas.addEventListener("pointerdown", (e) => {
  canvas.focus({ preventScroll: true });
  origin = canvas.getBoundingClientRect();
  send({
    kind: "pointer_down",
    id: e.pointerId,
    pointerType: e.pointerType,
    button: e.button,
    ...at(e),
    modifiers: modifiers(e),
    time: time(e),
  });
  // The map drags with the primary and secondary buttons.
  if (e.button === 0 || e.button === 2) canvas.setPointerCapture(e.pointerId);
});
// In a worker, the map draws as soon as a move arrives: pointerrawupdate
// comes when the browser has the move, pointermove only with the page's
// next frame. A touch moves with pointermove, which comes for all its
// fingers in one task, as one input message (`send`): Chromium dispatches
// each finger's pointerrawupdate in a task of its own, and the map would
// draw a pinch with one finger moved and not the other.
const raw = "onpointerrawupdate" in HTMLElement.prototype;
const move = (e: PointerEvent) =>
  send({ kind: "pointer_move", id: e.pointerId, ...at(e), time: time(e) });
if (raw) {
  canvas.addEventListener("pointerrawupdate", (e) => {
    if (e instanceof PointerEvent && e.pointerType !== "touch") move(e);
  });
}
canvas.addEventListener("pointermove", (e) => {
  if (!raw || e.pointerType === "touch") move(e);
});
canvas.addEventListener("pointerup", (e) =>
  send({ kind: "pointer_up", id: e.pointerId, ...at(e), time: time(e) }),
);
canvas.addEventListener("pointercancel", (e) =>
  send({ kind: "pointer_cancel", id: e.pointerId, time: time(e) }),
);
// wheelDelta (wheel.ts) reads the event's deltas first, `deltaMode` before
// `deltaY`, so that Firefox keeps a mouse wheel's lines.
canvas.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const delta = wheelDelta(e);
    if (delta.amount !== 0)
      send({
        kind: "wheel",
        ...at(e),
        ...delta,
        modifiers: modifiers(e),
        time: time(e),
      });
  },
  { passive: false },
);
// Shift and Ctrl change what a drag does (Shift draws a box, Ctrl rotates),
// so the map learns of them as they go down and up.
const MODIFIER_KEYS = new Set(["Shift", "Control"]);
const keyEvent = (e: KeyboardEvent) => {
  if (MODIFIER_KEYS.has(e.key))
    send({ kind: "modifiers", modifiers: modifiers(e), time: time(e) });
};
canvas.addEventListener("keydown", (e) => {
  keyEvent(e);
  if (KEYS.has(e.key)) e.preventDefault();
  if (KEYS.has(e.key) || e.key === "Escape")
    send({
      kind: "key",
      key: e.key,
      modifiers: modifiers(e),
      repeat: e.repeat,
      time: time(e),
    });
});
canvas.addEventListener("keyup", keyEvent);
canvas.addEventListener("blur", (e) => send({ kind: "blur", time: time(e) }));
canvas.addEventListener("contextmenu", (e) => e.preventDefault());

// A box-zoom drag's box, which the worker reports while it is drawn.
const box = element("box", HTMLElement);
function showBox(corners: Box | null) {
  box.hidden = !corners;
  if (!corners) return;
  const [x0, y0, x1, y1] = corners;
  Object.assign(box.style, {
    left: `${origin.left + window.scrollX + Math.min(x0, x1)}px`,
    top: `${origin.top + window.scrollY + Math.min(y0, y1)}px`,
    width: `${Math.abs(x1 - x0)}px`,
    height: `${Math.abs(y1 - y0)}px`,
  });
}

new ResizeObserver(() => {
  origin = canvas.getBoundingClientRect();
  const next: [number, number] = [mapBox.clientWidth, mapBox.clientHeight];
  if (next[0] === size[0] && next[1] === size[1]) return;
  size = next;
  fitCanvas();
  toMap({ type: "resize", width: size[0], height: size[1] });
  showDebugPage();
}).observe(mapBox);

// A window that moves to a display of another density draws at its pixel
// ratio, unless the page's parameters fix the ratio.
if (!params.has("ratio")) {
  const watchRatio = (current: number) => {
    matchMedia(`(resolution: ${current}dppx)`).addEventListener(
      "change",
      () => {
        ratio = window.devicePixelRatio || 1;
        fitCanvas();
        toMap({ type: "pixelRatio", ratio });
        showDebugPage();
        watchRatio(ratio);
      },
      { once: true },
    );
  };
  watchRatio(ratio);
}

// The camera in the hash, #zoom/lat/lon[/bearing/pitch], or null.
function cameraFromHash(hash: string): Camera | null {
  const parts = hash.slice(1).split("/").map(Number);
  if (parts.some((part) => !Number.isFinite(part))) return null;
  const [zoom, lat, lon, bearing = 0, pitch = 0, ...rest] = parts;
  if (
    zoom === undefined ||
    lat === undefined ||
    lon === undefined ||
    rest.length > 0
  )
    return null;
  return [lat, lon, zoom, bearing, pitch];
}

function hashFromCamera([lat, lon, zoom, bearing, pitch]: Camera): string {
  // Enough digits for a pixel at this zoom, as GL JS writes them.
  const digits = Math.max(
    0,
    Math.ceil(Math.log10((2 ** zoom * 512) / 360 / 0.5)),
  );
  const round = (value: number, places: number) =>
    String(Number(value.toFixed(places)));
  let hash = `#${round(zoom, 2)}/${round(lat, digits)}/${round(lon, digits)}`;
  if (Math.abs(bearing) >= 0.05 || Math.abs(pitch) >= 0.5)
    hash += `/${round(bearing, 1)}/${round(pitch, 0)}`;
  return hash;
}

// The hash follows the camera, written at most every 300 ms (browsers limit
// how often a page may replace its URL).
let writtenHash = location.hash;
let hashTimer: ReturnType<typeof setTimeout> | null = null;
let latestCamera = camera;
const needle = element("needle", SVGElement);
function showCamera(next: Camera) {
  latestCamera = next;
  const [, , , bearing, pitch] = next;
  needle.style.transform = `rotateX(${pitch}deg) rotateZ(${-bearing}deg)`;
  debug.camera(next);
  comparison?.cameraChanged(next);
  hashTimer ??= setTimeout(() => {
    hashTimer = null;
    writtenHash = hashFromCamera(latestCamera);
    history.replaceState(history.state, "", writtenHash);
  }, 300);
}
window.addEventListener("hashchange", () => {
  if (location.hash === writtenHash) return;
  const next = cameraFromHash(location.hash);
  if (next) toMap({ type: "jumpTo", camera: next });
});

// Zoom and the compass.
element("zoom-in", HTMLButtonElement).addEventListener("click", (e) => {
  comparison?.lead();
  send({ kind: "key", key: "+", modifiers: 0, repeat: false, time: time(e) });
});
element("zoom-out", HTMLButtonElement).addEventListener("click", (e) => {
  comparison?.lead();
  send({ kind: "key", key: "-", modifiers: 0, repeat: false, time: time(e) });
});
element("compass", HTMLButtonElement).addEventListener("click", () => {
  comparison?.lead();
  toMap({ type: "resetNorth" });
});

// The style button and its list.
const styleList = element("style-list", HTMLElement);
let loading = true;
let styleName = "";
function showStyle() {
  const name = current.key === null ? styleName || current.name : current.name;
  element("style-name", HTMLElement).textContent = name;
  const detail = element("style-detail", HTMLElement);
  detail.textContent = fatal
    ? "Stopped"
    : loading
      ? "Loading…"
      : current.provider === name
        ? ""
        : current.provider;
  detail.hidden = detail.textContent === "";
  document.documentElement.dataset.theme = current.dark ? "dark" : "light";
  for (const option of styleList.querySelectorAll<HTMLElement>(
    ".style-option",
  )) {
    option.setAttribute(
      "aria-current",
      String(option.dataset.key === current.key),
    );
  }
}
function pickStyle(next: Style) {
  element("styles", HTMLElement).hidePopover();
  if (sameStyle(next, current)) return;
  current = next;
  loading = true;
  styleName = "";
  showStyle();
  showAttribution([]);
  toMap({ type: "style", style: next.source, terrain: next.terrain });
  comparison?.setStyle(next);
  setParam("style", next.key);
}
// Sets (or, for null, removes) a URL parameter, keeping the camera's hash.
function setParam(name: string, value: string | null) {
  const url = new URL(location.href);
  if (value === null) url.searchParams.delete(name);
  else url.searchParams.set(name, value);
  url.hash = writtenHash;
  history.replaceState(history.state, "", url);
}
// Group styles by provider, collecting single-style providers under "Other".
const providerCounts = new Map<string, number>();
for (const option of STYLES) {
  providerCounts.set(
    option.provider,
    (providerCounts.get(option.provider) ?? 0) + 1,
  );
}
const styleGroups = new Map<string, Style[]>();
for (const option of STYLES) {
  const heading =
    (providerCounts.get(option.provider) ?? 0) > 1 ? option.provider : "Other";
  const group = styleGroups.get(heading);
  if (group) group.push(option);
  else styleGroups.set(heading, [option]);
}
styleList.replaceChildren(
  ...[...styleGroups].map(([heading, options]) => {
    const title = document.createElement("div");
    title.className = "style-group-title";
    title.textContent = heading;
    const list = document.createElement("ul");
    list.className = "style-group-list";
    list.append(
      ...options.map((option) => {
        const name = document.createElement("span");
        name.className = "style-name";
        name.textContent = option.name;
        const text = document.createElement("span");
        text.className = "style-text";
        text.append(name);
        if (heading === "Other" && option.provider !== option.name) {
          const provider = document.createElement("span");
          provider.className = "style-detail";
          provider.textContent = option.provider;
          text.append(provider);
        }
        const button = document.createElement("button");
        button.type = "button";
        button.className = "style-option";
        button.dataset.key = option.key ?? "";
        button.append(text);
        button.addEventListener("click", () => pickStyle(option));
        const item = document.createElement("li");
        item.append(button);
        return item;
      }),
    );
    const section = document.createElement("section");
    section.className = "style-group";
    section.append(title, list);
    return section;
  }),
);
showStyle();

// Attribution: the sources' HTML, kept to its text and its http(s) links.
const attribution = element("attribution", HTMLElement);
const attributionToggle = element("attribution-toggle", HTMLButtonElement);
attributionToggle.addEventListener("click", () => {
  const open = !attribution.classList.contains("open");
  attribution.classList.toggle("open", open);
  attributionToggle.setAttribute("aria-expanded", String(open));
});
function sanitize(node: Node, into: HTMLElement) {
  for (const child of node.childNodes) {
    if (child instanceof Text) {
      into.append(child.data);
    } else if (
      child instanceof HTMLAnchorElement &&
      /^https?:\/\//.test(child.getAttribute("href") ?? "")
    ) {
      const link = document.createElement("a");
      link.href = child.href;
      link.target = "_blank";
      link.rel = "noopener";
      link.textContent = child.textContent;
      into.append(link);
    } else if (child instanceof Element) {
      sanitize(child, into);
    }
  }
}
function showAttribution(entries: string[]) {
  const all = entries.map((entry) => {
    const part = document.createElement("span");
    sanitize(new DOMParser().parseFromString(entry, "text/html").body, part);
    return { part, text: part.textContent.trim() };
  });
  // Each credit once: an entry is left out when a longer one, or an equal
  // one before it, contains its text.
  const repeated = (i: number, text: string) =>
    all.some(
      (other, j) =>
        j !== i &&
        other.text.includes(text) &&
        (other.text.length > text.length || j < i),
    );
  const parts = all
    .filter(({ text }, i) => text && !repeated(i, text))
    .map(({ part }) => part);
  element("attribution-text", HTMLElement).replaceChildren(
    ...parts.flatMap((part, i) => (i ? [" ", part] : [part])),
  );
  attribution.hidden = parts.length === 0;
}

// The comparison with GL JS (compare.ts), which loads when it first opens.
let comparison: Comparison | null = null;
let comparing = false;
// The projection in place of the style's (setProjection), or null.
let currentProjection: ProjectionType | null = null;
const compareToggle = element("compare-toggle", HTMLInputElement);
async function setComparing(on: boolean) {
  if (on === comparing) return;
  comparing = on;
  compareToggle.checked = on;
  setParam("compare", on ? "" : null);
  if (!on) {
    comparison?.remove();
    comparison = null;
    return;
  }
  try {
    const { compare } = await import("./compare.ts");
    const style = current;
    const projection = currentProjection;
    const opened = compare(canvas, {
      style,
      projection,
      camera: latestCamera,
      follow: (next) => toMap({ type: "follow", camera: next }),
      stop: () => toMap({ type: "stop" }),
    });
    // Closed, or the style changed, while GL JS loaded.
    if (!comparing) {
      opened.remove();
      return;
    }
    comparison = opened;
    if (!sameStyle(current, style)) comparison.setStyle(current);
    if (currentProjection !== projection)
      comparison.setProjection(currentProjection);
  } catch (error) {
    console.error("the comparison failed to open:", error);
    if (comparing) setComparing(false);
  }
}
compareToggle.addEventListener("change", () =>
  setComparing(compareToggle.checked),
);
if (params.has("compare")) setComparing(true);

// Free bearing on the globe, off as the map starts.
const freeBearingToggle = element("free-bearing-toggle", HTMLInputElement);
function setFreeBearing(on: boolean) {
  freeBearingToggle.checked = on;
  setParam("freeBearing", on ? "" : null);
  toMap({ type: "globeFreeBearing", on });
}
freeBearingToggle.addEventListener("change", () =>
  setFreeBearing(freeBearingToggle.checked),
);
if (params.has("freeBearing")) setFreeBearing(true);

// The projection, in place of the style's: `?projection=`, a key of
// PROJECTIONS; the style's own without one.
const PROJECTIONS: Record<string, ProjectionType | null> = {
  style: null,
  flat: "mercator",
  globe: "vertical-perspective",
  handoff: "globe",
};
const projectionSelect = element("projection-select", HTMLSelectElement);
function setProjection(key: string) {
  const projection = PROJECTIONS[key] ?? null;
  projectionSelect.value = projection === null ? "style" : key;
  setParam("projection", projection === null ? null : key);
  currentProjection = projection;
  toMap({ type: "projection", projection });
  comparison?.setProjection(projection);
}
projectionSelect.addEventListener("change", () =>
  setProjection(projectionSelect.value),
);
const projectionParam = params.get("projection");
if (projectionParam !== null) setProjection(projectionParam);

// The backend control, at the bottom left: the backend's logo and name, and a
// button to load the page again with the other backend where it can draw.
const backendButton = element("backend-button", HTMLButtonElement);
element("backend-name", HTMLElement).textContent = BACKEND_NAME[backend];
for (const [id, shown] of [
  ["backend-logo-webgpu", backend === "webgpu"],
  ["backend-logo-webgl", backend === "webgl2"],
] as const)
  element(id, SVGElement).toggleAttribute("hidden", !shown);
// The attribution keeps clear of the control (page.css).
new ResizeObserver(() =>
  document.documentElement.style.setProperty(
    "--backend-width",
    `${backendButton.offsetWidth}px`,
  ),
).observe(backendButton);
backendButton.addEventListener("click", () => {
  const url = new URL(location.href);
  url.searchParams.set("backend", OTHER_BACKEND[backend]);
  url.hash = hashFromCamera(latestCamera);
  location.assign(url);
});
async function otherBackendCanDraw(): Promise<boolean> {
  const other = OTHER_BACKEND[backend];
  if (!canDraw(other)) return false;
  // A browser can have WebGPU's API but no adapter for it, as Chromium on
  // Linux often does; WebGL2 that fails falls back to WebGPU.
  if (other !== "webgpu") return true;
  try {
    const options = { powerPreference: "high-performance" } as const;
    return (await navigator.gpu.requestAdapter(options)) !== null;
  } catch {
    return false;
  }
}
void otherBackendCanDraw().then((can) => {
  if (!can) return;
  backendButton.disabled = false;
  backendButton.title = `Switch to ${BACKEND_NAME[OTHER_BACKEND[backend]]}`;
});

// The debug overlay (debug.ts), off as the page starts unless `?debug`.
const debugToggle = element("debug-toggle", HTMLInputElement);
function showDebugPage() {
  debug.page({
    backend: BACKEND_NAME[backend],
    width: size[0],
    height: size[1],
    ratio,
  });
}
function setDebug(on: boolean) {
  debugToggle.checked = on;
  setParam("debug", on ? "" : null);
  debug.show(on);
  toMap({ type: "debug", on });
  if (on) {
    showDebugPage();
    debug.camera(latestCamera);
  }
}
debugToggle.addEventListener("change", () => setDebug(debugToggle.checked));
// D, unless a modifier makes it another shortcut or a field takes the text.
window.addEventListener("keydown", (e) => {
  if (
    e.key.toLowerCase() !== "d" ||
    e.repeat ||
    e.ctrlKey ||
    e.metaKey ||
    e.altKey
  )
    return;
  if (
    e.target instanceof HTMLElement &&
    e.target.closest("input, textarea, select, [contenteditable]")
  )
    return;
  setDebug(!debug.shown);
});
if (params.has("debug")) setDebug(true);
