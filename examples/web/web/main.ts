// The page: it runs no wasm itself. It compiles the module, gives its canvas
// to the map's Web Worker (map-worker.ts) as an OffscreenCanvas, and starts
// the tile workers' Web Workers (worker.ts) while the module compiles, each
// connected to the map's by a MessageChannel, so that their requests and
// replies never pass through this thread. Each Web Worker instantiates the
// same module with its own memory, so nothing needs SharedArrayBuffer or
// COOP/COEP headers.
//
// The map draws with WebGPU. A browser without it gets the module's other
// build, which draws with WebGL2 (pkg/webgl, worker-webgl.ts). So does one
// whose WebGPU can't start the map: the page then loads again with WebGL2,
// and the tab remembers that.
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
//   width, height, ratio   a fixed canvas size in CSS pixels and pixel ratio
//                          (default: the window and devicePixelRatio)
//   localIdeographFontFamily=<CSS font-family list>
//                          the font CJK glyphs are drawn in rather than
//                          downloaded, as GL JS's option (default:
//                          sans-serif); empty or `false` downloads them
//   compare                MapLibre GL JS's map of the style beside this one
//                          (compare.ts)
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
// +/= and - keys. The canvas follows its size (ResizeObserver). The zoom
// buttons send the + and - keys; the compass eases the bearing and pitch
// back to zero.
//
// `window.mln` is for scripts (check/check.mjs): `backend` is what the map
// draws with, `loaded` resolves with the
// load time and the number of worker replies once the map is fully loaded,
// and `readPixels()` draws the frame again and resolves with its RGBA
// pixels, read back from the GPU. `fetches()` resolves with every URL the
// map's Web Worker fetched and how many times, and `idle()` with the camera
// once the map is fully loaded and has stopped moving. A panic in any of
// the Web Workers' wasm instances stops the map: the page shows its
// message, `fatal` resolves with it as an Error, and `loaded` and the
// methods reject with it.

import wasmUrl from './pkg/mln_web_bg.wasm?url';
import webglWasmUrl from './pkg/webgl/mln_web_bg.wasm?url';
import type { Comparison } from './compare.ts';
import {
  type Backend,
  type Box,
  type Camera,
  type FromMap,
  type Input,
  type Loaded,
  type Pixels,
  type StartTileWorker,
  type ToMap,
  WHEEL_NOTCHES,
  WHEEL_PIXELS,
} from './protocol.ts';
import { STYLES, type Style, styleFor } from './styles.ts';

declare global {
  interface Window {
    mln: {
      backend: Backend;
      loaded: Promise<Loaded>;
      fatal: Promise<Error>;
      readPixels(): Promise<Pixels>;
      fetches(): Promise<Record<string, number>>;
      idle(): Promise<Camera>;
    };
  }
}

/** The element with `id`, which must be a `type`. */
function element<T extends Element>(id: string, type: abstract new (...args: never[]) => T): T {
  const found = document.getElementById(id);
  if (!(found instanceof type)) throw new Error(`#${id} is not a ${type.name}`);
  return found;
}

const params = new URLSearchParams(location.search);
const number = (name: string, fallback: number) => (params.has(name) ? Number(params.get(name)) : fallback);
const status = element('status', HTMLElement);
const banner = element('error', HTMLElement);
const canvas = element('map', HTMLCanvasElement);

if (params.has('width') && params.has('height')) {
  canvas.style.width = `${number('width', 0)}px`;
  canvas.style.height = `${number('height', 0)}px`;
}
const ratio = number('ratio', window.devicePixelRatio || 1);
canvas.width = Math.round(canvas.clientWidth * ratio);
canvas.height = Math.round(canvas.clientHeight * ratio);

const localFamily = params.get('localIdeographFontFamily') ?? 'sans-serif';

// What a tab whose WebGPU couldn't start the map draws with from then on.
const BACKEND_KEY = 'mln-backend';
function rememberedBackend(): string | null {
  try {
    return sessionStorage.getItem(BACKEND_KEY);
  } catch {
    return null; // no storage for this page
  }
}
const backend: Backend =
  (params.get('backend') ?? rememberedBackend() ?? ('gpu' in navigator ? 'webgpu' : 'webgl2')) === 'webgl2'
    ? 'webgl2'
    : 'webgpu';

const compiling = WebAssembly.compileStreaming(fetch(backend === 'webgl2' ? webglWasmUrl : wasmUrl));

// The Web Workers load their scripts while the module downloads and
// compiles, and get it once it has.
const count = number('workers', Math.max(1, Math.min(4, (navigator.hardwareConcurrency || 2) - 1)));
const tileWorkers = Array.from({ length: count }, () =>
  backend === 'webgl2'
    ? new Worker(new URL('./worker-webgl.ts', import.meta.url), { type: 'module' })
    : new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }));
const map = backend === 'webgl2'
  ? new Worker(new URL('./map-worker.ts', import.meta.url), { type: 'module', name: 'webgl2' })
  : new Worker(new URL('./map-worker.ts', import.meta.url), { type: 'module' });
const toMap = (message: ToMap, transfer: Transferable[] = []) => map.postMessage(message, transfer);
const camera: Camera = cameraFromHash(location.hash) ??
  [number('lat', 52.37), number('lon', 4.89), number('zoom', 13), number('bearing', 0), number('pitch', 0)];
let current = styleFor(params.get('style'));
const style = current.url;
// The map's Web Worker fetches the style and what it needs first meanwhile.
toMap({ type: 'preload', style, ratio, zoom: camera[2] });

const panic = params.get('panic');
const panicMessage = 'panic=<map|tile> asked for this panic';

// The rest of the page starts meanwhile: its controls work at once, and
// what they ask of the map waits in the map's Web Worker until it starts.
compiling.then((module) => {
  const ports = tileWorkers.map((worker, i) => {
    const channel = new MessageChannel();
    const start: StartTileWorker = {
      module,
      port: channel.port2,
      panic: panic === 'tile' && i === 0 ? panicMessage : undefined,
    };
    worker.postMessage(start, [channel.port2]);
    return channel.port1;
  });

  const offscreen = canvas.transferControlToOffscreen();
  toMap({
    type: 'start',
    module,
    canvas: offscreen,
    ports,
    width: canvas.clientWidth,
    height: canvas.clientHeight,
    ratio,
    style,
    fallbackFontFaces: params.get('fallbackFontFaces'),
    camera,
    localIdeographFontFamily: localFamily === '' || localFamily === 'false' ? undefined : localFamily,
    panic: panic === 'map' ? panicMessage : undefined,
  }, [offscreen, ...ports]);
}, (error: unknown) => stop(`the module didn't compile: ${error instanceof Error ? error.message : String(error)}`));

function showBanner(text: string) {
  banner.textContent = text;
  banner.hidden = false;
}

// The browser's API couldn't start the map. After WebGPU the page loads
// again to draw with WebGL2, in this tab from now on; after WebGL2 there is
// nothing left to draw with.
function unsupported(message: string) {
  if (backend === 'webgl2') {
    stop(`this browser has neither WebGPU nor WebGL2 for the map to draw with (${message})`);
    return;
  }
  try {
    sessionStorage.setItem(BACKEND_KEY, 'webgl2');
    location.reload();
  } catch {
    const url = new URL(location.href);
    url.searchParams.set('backend', 'webgl2');
    location.replace(url);
  }
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
  readPixels: () => ask(pixelRequests, { type: 'readPixels' }),
  fetches: () => ask(fetchesRequests, { type: 'fetches' }),
  idle: () => ask(idleRequests, { type: 'idle' }),
};
// Rejected on a panic, which the page reports itself.
window.mln.loaded.catch(() => {});

map.onmessage = ({ data }: MessageEvent<FromMap>) => {
  switch (data.type) {
    case 'box':
      showBox(data.box);
      break;
    case 'status':
      status.textContent = data.text;
      break;
    case 'loaded':
      markLoaded(data.loaded);
      loading = false;
      showStyle();
      break;
    case 'pixels':
      pixelRequests.shift()?.resolve(data.pixels);
      break;
    case 'fetches':
      fetchesRequests.shift()?.resolve(data.fetches);
      break;
    case 'idle':
      idleRequests.shift()?.resolve(data.camera);
      break;
    case 'fatal':
      stop(data.message);
      break;
    case 'unsupported':
      unsupported(data.message);
      break;
    case 'camera':
      showCamera(data.camera);
      break;
    case 'attribution':
      showAttribution(data.attribution);
      break;
    case 'styleName':
      styleName = data.name;
      showStyle();
      break;
  }
};

function stop(message: string) {
  const error = new Error(message);
  fatal = error;
  console.error(`the map stopped: ${message}`);
  showBanner(`The map stopped: ${message}`);
  status.textContent = 'stopped';
  showStyle();
  failLoaded(error);
  markFatal(error);
  for (const queue of [pixelRequests, fetchesRequests, idleRequests]) {
    for (const request of queue.splice(0)) request.reject(error);
  }
}

const send = (input: Input) => toMap({ type: 'input', input });
// Each event's time, as performance.timeOrigin + timeStamp: the map's Web
// Worker turns it into its own performance.now(), the clock its frames run on.
const time = (e: Event) => performance.timeOrigin + e.timeStamp;
const modifiers = (e: MouseEvent | KeyboardEvent) =>
  (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
// The keys the map uses; Escape only ends a box zoom, so its default stays.
const KEYS = new Set(['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', '+', '=', '-', '_']);
// Where the canvas is: read on each press and resize rather than on every
// move, which would lay the page out again.
let origin = canvas.getBoundingClientRect();
const at = (e: MouseEvent) => ({ x: e.clientX - origin.left, y: e.clientY - origin.top });

canvas.tabIndex = 0;
canvas.style.touchAction = 'none';
canvas.style.outline = 'none';
canvas.addEventListener('pointerdown', (e) => {
  canvas.focus({ preventScroll: true });
  origin = canvas.getBoundingClientRect();
  send({
    kind: 'pointer_down',
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
// next frame.
const moveEvent = 'onpointerrawupdate' in HTMLElement.prototype ? 'pointerrawupdate' : 'pointermove';
canvas.addEventListener(moveEvent, (e) => {
  if (e instanceof PointerEvent) send({ kind: 'pointer_move', id: e.pointerId, ...at(e), time: time(e) });
});
canvas.addEventListener('pointerup', (e) => send({ kind: 'pointer_up', id: e.pointerId, ...at(e), time: time(e) }));
canvas.addEventListener('pointercancel', (e) => send({ kind: 'pointer_cancel', id: e.pointerId, time: time(e) }));
// A wheel event as the recognizer takes it (WebMap::wheel's `kind`):
// WHEEL_NOTCHES and the notches for a mouse wheel's detents, which it eases,
// or WHEEL_PIXELS and the pixels for continuous scrolling (a trackpad, a
// precise wheel), which it applies at once. Positive scrolls down. Browsers
// don't say which device scrolled, so this reads it from how they report
// each:
// - Firefox reports wheels in lines, 3 a notch, and pages.
// - Safari reports a notch as 4.000244140625 px, and a faster turn as a
//   whole multiple of it (GL JS's test): one notch per multiple.
// - Chromium's legacy wheelDeltaY is 120 a detent, and -3 × deltaY for
//   precise scrolling.
// Untested on real hardware; a misread zooms the same way by a similar
// amount, with or without the easing.
const SAFARI_NOTCH = 4.000244140625;
function wheelDelta(e: WheelEvent): { wheel: number; amount: number } {
  if (e.deltaMode === WheelEvent.DOM_DELTA_LINE) return { wheel: WHEEL_NOTCHES, amount: e.deltaY / 3 };
  if (e.deltaMode === WheelEvent.DOM_DELTA_PAGE) return { wheel: WHEEL_NOTCHES, amount: e.deltaY };
  if (e.deltaY !== 0 && e.deltaY % SAFARI_NOTCH === 0) return { wheel: WHEEL_NOTCHES, amount: e.deltaY / SAFARI_NOTCH };
  const legacy = 'wheelDeltaY' in e ? e.wheelDeltaY : undefined;
  if (typeof legacy === 'number' && legacy !== 0 && Math.abs(legacy + 3 * e.deltaY) >= 1 && legacy % 120 === 0) {
    return { wheel: WHEEL_NOTCHES, amount: -legacy / 120 };
  }
  return { wheel: WHEEL_PIXELS, amount: e.deltaY };
}
canvas.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (e.deltaY !== 0) send({ kind: 'wheel', ...at(e), ...wheelDelta(e), modifiers: modifiers(e), time: time(e) });
}, { passive: false });
canvas.addEventListener('keydown', (e) => {
  if (KEYS.has(e.key)) e.preventDefault();
  if (KEYS.has(e.key) || e.key === 'Escape') send({ kind: 'key', key: e.key, modifiers: modifiers(e), time: time(e) });
});
canvas.addEventListener('blur', (e) => send({ kind: 'blur', time: time(e) }));
canvas.addEventListener('contextmenu', (e) => e.preventDefault());

// A Shift-drag's box, which the worker reports while it is drawn.
const box = element('box', HTMLElement);
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

// The canvas's size in CSS pixels; the worker resizes the map and its
// surface. Its first report is the size the map started at.
let size = [canvas.clientWidth, canvas.clientHeight];
new ResizeObserver(() => {
  origin = canvas.getBoundingClientRect();
  const next = [canvas.clientWidth, canvas.clientHeight];
  if (next[0] === size[0] && next[1] === size[1]) return;
  size = next;
  toMap({ type: 'resize', width: canvas.clientWidth, height: canvas.clientHeight });
}).observe(canvas);

// The camera in the hash, #zoom/lat/lon[/bearing/pitch], or null.
function cameraFromHash(hash: string): Camera | null {
  const parts = hash.slice(1).split('/').map(Number);
  if (parts.some((part) => !Number.isFinite(part))) return null;
  const [zoom, lat, lon, bearing = 0, pitch = 0, ...rest] = parts;
  if (zoom === undefined || lat === undefined || lon === undefined || rest.length > 0) return null;
  return [lat, lon, zoom, bearing, pitch];
}

function hashFromCamera([lat, lon, zoom, bearing, pitch]: Camera): string {
  // Enough digits for a pixel at this zoom, as GL JS writes them.
  const digits = Math.max(0, Math.ceil(Math.log10(2 ** zoom * 512 / 360 / 0.5)));
  const round = (value: number, places: number) => String(Number(value.toFixed(places)));
  let hash = `#${round(zoom, 2)}/${round(lat, digits)}/${round(lon, digits)}`;
  if (Math.abs(bearing) >= 0.05 || Math.abs(pitch) >= 0.5) hash += `/${round(bearing, 1)}/${round(pitch, 0)}`;
  return hash;
}

// The hash follows the camera, written at most every 300 ms (browsers limit
// how often a page may replace its URL).
let writtenHash = location.hash;
let hashTimer: ReturnType<typeof setTimeout> | null = null;
let latestCamera = camera;
const needle = element('needle', SVGElement);
function showCamera(next: Camera) {
  latestCamera = next;
  const [, , , bearing, pitch] = next;
  needle.style.transform = `rotateX(${pitch}deg) rotateZ(${-bearing}deg)`;
  comparison?.cameraChanged(next);
  hashTimer ??= setTimeout(() => {
    hashTimer = null;
    writtenHash = hashFromCamera(latestCamera);
    history.replaceState(history.state, '', writtenHash);
  }, 300);
}
window.addEventListener('hashchange', () => {
  if (location.hash === writtenHash) return;
  const next = cameraFromHash(location.hash);
  if (next) toMap({ type: 'jumpTo', camera: next });
});

// Zoom and the compass.
element('zoom-in', HTMLButtonElement).addEventListener('click', (e) => {
  comparison?.lead();
  send({ kind: 'key', key: '+', modifiers: 0, time: time(e) });
});
element('zoom-out', HTMLButtonElement).addEventListener('click', (e) => {
  comparison?.lead();
  send({ kind: 'key', key: '-', modifiers: 0, time: time(e) });
});
element('compass', HTMLButtonElement).addEventListener('click', () => {
  comparison?.lead();
  toMap({ type: 'resetNorth' });
});

// The style button and its list.
const styleList = element('style-list', HTMLElement);
let loading = true;
let styleName = '';
function showStyle() {
  element('style-name', HTMLElement).textContent = current.key === null ? styleName || current.name : current.name;
  element('style-detail', HTMLElement).textContent = fatal ? 'Stopped' : loading ? 'Loading…' : current.provider;
  document.documentElement.dataset.theme = current.dark ? 'dark' : 'light';
  for (const option of styleList.querySelectorAll<HTMLElement>('.style-option')) {
    option.setAttribute('aria-current', String(option.dataset.key === current.key));
  }
}
function pickStyle(next: Style) {
  element('styles', HTMLElement).hidePopover();
  if (next.url === current.url) return;
  current = next;
  loading = true;
  styleName = '';
  showStyle();
  showAttribution([]);
  toMap({ type: 'style', url: next.url });
  comparison?.setStyle(next.url);
  setParam('style', next.key);
}
// Sets (or, for null, removes) a URL parameter, keeping the camera's hash.
function setParam(name: string, value: string | null) {
  const url = new URL(location.href);
  if (value === null) url.searchParams.delete(name);
  else url.searchParams.set(name, value);
  url.hash = writtenHash;
  history.replaceState(history.state, '', url);
}
styleList.replaceChildren(...STYLES.map((option) => {
  const name = document.createElement('span');
  name.className = 'style-name';
  name.textContent = option.name;
  const provider = document.createElement('span');
  provider.className = 'style-detail';
  provider.textContent = option.provider;
  const text = document.createElement('span');
  text.className = 'style-text';
  text.append(name, provider);
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'style-option';
  button.dataset.key = option.key ?? '';
  button.append(text);
  button.addEventListener('click', () => pickStyle(option));
  const item = document.createElement('li');
  item.append(button);
  return item;
}));
showStyle();

// Attribution: the sources' HTML, kept to its text and its http(s) links.
const attribution = element('attribution', HTMLElement);
const attributionToggle = element('attribution-toggle', HTMLButtonElement);
attributionToggle.addEventListener('click', () => {
  const open = !attribution.classList.contains('open');
  attribution.classList.toggle('open', open);
  attributionToggle.setAttribute('aria-expanded', String(open));
});
function sanitize(node: Node, into: HTMLElement) {
  for (const child of node.childNodes) {
    if (child instanceof Text) {
      into.append(child.data);
    } else if (child instanceof HTMLAnchorElement && /^https?:\/\//.test(child.getAttribute('href') ?? '')) {
      const link = document.createElement('a');
      link.href = child.href;
      link.target = '_blank';
      link.rel = 'noopener';
      link.textContent = child.textContent;
      into.append(link);
    } else if (child instanceof Element) {
      sanitize(child, into);
    }
  }
}
function showAttribution(entries: string[]) {
  const all = entries.map((entry) => {
    const part = document.createElement('span');
    sanitize(new DOMParser().parseFromString(entry, 'text/html').body, part);
    return { part, text: part.textContent.trim() };
  });
  // Each credit once: an entry is left out when a longer one, or an equal
  // one before it, contains its text.
  const repeated = (i: number, text: string) => all.some((other, j) =>
    j !== i && other.text.includes(text) && (other.text.length > text.length || j < i));
  const parts = all.filter(({ text }, i) => text && !repeated(i, text)).map(({ part }) => part);
  element('attribution-text', HTMLElement).replaceChildren(...parts.flatMap((part, i) => (i ? [' ', part] : [part])));
  attribution.hidden = parts.length === 0;
}

// The comparison with GL JS (compare.ts), which loads when it first opens.
let comparison: Comparison | null = null;
let comparing = false;
const compareToggle = element('compare-toggle', HTMLInputElement);
async function setComparing(on: boolean) {
  if (on === comparing) return;
  comparing = on;
  compareToggle.checked = on;
  setParam('compare', on ? '' : null);
  if (!on) {
    comparison?.remove();
    comparison = null;
    return;
  }
  try {
    const { compare } = await import('./compare.ts');
    const style = current.url;
    const opened = compare(canvas, {
      style,
      camera: latestCamera,
      follow: (next) => toMap({ type: 'follow', camera: next }),
      stop: () => toMap({ type: 'stop' }),
    });
    // Closed, or the style changed, while GL JS loaded.
    if (!comparing) {
      opened.remove();
      return;
    }
    comparison = opened;
    if (current.url !== style) comparison.setStyle(current.url);
  } catch (error) {
    console.error('the comparison failed to open:', error);
    if (comparing) setComparing(false);
  }
}
compareToggle.addEventListener('change', () => setComparing(compareToggle.checked));
if (params.has('compare')) setComparing(true);
