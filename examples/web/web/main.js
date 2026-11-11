// The page: it runs no wasm itself. It compiles the module, gives its canvas
// to the map's Web Worker (map-worker.js) as an OffscreenCanvas, and starts
// the tile workers' Web Workers (worker.js) while the module compiles, each
// connected to the map's by a MessageChannel, so that their requests and replies never pass through
// this thread. Each Web Worker instantiates the same module with its own
// memory, so nothing needs SharedArrayBuffer or COOP/COEP headers.
//
// URL parameters:
//   style=<key|url>        a style of styles.js, or any style's URL
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
//   panic=<map|tile>       the map's instance or the first tile worker's
//                          panics once it starts, for check/check.mjs
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
// +/= and - keys. Each event carries its time as performance.timeOrigin +
// timeStamp, which the worker turns into its own performance.now() clock,
// the one its frames run on. The canvas follows its size (ResizeObserver).
// The zoom buttons send the + and - keys; the compass eases the bearing and
// pitch back to zero.
//
// `window.mln` is for scripts (check/check.mjs): `loaded` resolves with the
// load time and the number of worker replies once the map is fully loaded,
// and `readPixels()` draws the frame again and resolves with its RGBA
// pixels, read back from the GPU. A panic in any of the Web Workers' wasm
// instances stops the map: the page shows its message, `fatal` resolves
// with it as an Error, and `loaded` and `readPixels()` reject with it.

import { STYLES, styleFor } from './styles.js';

const params = new URLSearchParams(location.search);
const number = (name, fallback) => (params.has(name) ? Number(params.get(name)) : fallback);
const $ = (id) => document.getElementById(id);
const status = $('status');
const banner = $('error');
const canvas = document.getElementById('map');

if (params.has('width') && params.has('height')) {
  canvas.style.width = `${number('width')}px`;
  canvas.style.height = `${number('height')}px`;
}
const ratio = number('ratio', window.devicePixelRatio || 1);
canvas.width = Math.round(canvas.clientWidth * ratio);
canvas.height = Math.round(canvas.clientHeight * ratio);

const localFamily = params.get('localIdeographFontFamily') ?? 'sans-serif';

const compiling = WebAssembly.compileStreaming(fetch('./pkg/mln_web_bg.wasm'));

// The Web Workers load their scripts while the module downloads and
// compiles, and get it once it has.
const count = number('workers', Math.max(1, Math.min(4, (navigator.hardwareConcurrency || 2) - 1)));
const tileWorkers = Array.from({ length: count }, () => new Worker('./worker.js', { type: 'module' }));
const map = new Worker('./map-worker.js', { type: 'module' });
const camera = cameraFromHash(location.hash) ??
  [number('lat', 52.37), number('lon', 4.89), number('zoom', 13), number('bearing', 0), number('pitch', 0)];
let current = styleFor(params.get('style'));
const style = current.url;
// The map's Web Worker fetches the style and what it needs first meanwhile.
map.postMessage({ preload: { style, ratio, zoom: camera[2] } });

const panic = params.get('panic');
const panicMessage = 'panic=<map|tile> asked for this panic';

// The rest of the page starts meanwhile: its controls work at once, and
// what they ask of the map waits in the map's Web Worker until it starts.
compiling.then((module) => {
  const ports = tileWorkers.map((worker, i) => {
    const channel = new MessageChannel();
    worker.postMessage({ module, port: channel.port2, panic: panic === 'tile' && i === 0 ? panicMessage : undefined },
      [channel.port2]);
    return channel.port1;
  });

  const offscreen = canvas.transferControlToOffscreen();
  map.postMessage({
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
}, (error) => stop(`the module didn't compile: ${error.message}`));

function showBanner(text) {
  banner.textContent = text;
  banner.hidden = false;
}
if (!navigator.gpu) {
  showBanner('This browser has no WebGPU, which the map draws with. Try a recent Chrome, Edge or Safari, or Firefox 141 or later on Windows.');
}

let markLoaded;
let failLoaded;
let markFatal;
let fatal = null;
const pixelRequests = [];
window.mln = {
  loaded: new Promise((resolve, reject) => {
    markLoaded = resolve;
    failLoaded = reject;
  }),
  fatal: new Promise((resolve) => { markFatal = resolve; }),
  readPixels() {
    return new Promise((resolve, reject) => {
      if (fatal) {
        reject(fatal);
        return;
      }
      pixelRequests.push({ resolve, reject });
      map.postMessage({ readPixels: true });
    });
  },
};
// Rejected on a panic, which the page reports itself.
window.mln.loaded.catch(() => {});

map.onmessage = ({ data }) => {
  if (data.box !== undefined) showBox(data.box);
  if (data.status !== undefined) status.textContent = data.status;
  if (data.loaded) {
    markLoaded(data.loaded);
    loading = false;
    showStyle();
  }
  if (data.pixels) pixelRequests.shift()?.resolve(data.pixels);
  if (data.fatal !== undefined) stop(data.fatal);
  if (data.camera) showCamera(data.camera);
  if (data.attribution) showAttribution(data.attribution);
  if (data.styleName !== undefined) {
    styleName = data.styleName;
    showStyle();
  }
};

function stop(message) {
  fatal = new Error(message);
  console.error(`the map stopped: ${message}`);
  showBanner(`The map stopped: ${message}`);
  status.textContent = 'stopped';
  showStyle();
  failLoaded(fatal);
  markFatal(fatal);
  for (const request of pixelRequests.splice(0)) request.reject(fatal);
}

// The map's input. Positions are CSS pixels from the canvas's top-left
// corner; modifiers are bits: Shift 1, Ctrl 2, Alt 4, Meta 8.
const send = (input, ...args) => map.postMessage({ input, args });
const time = (e) => performance.timeOrigin + e.timeStamp;
const modifiers = (e) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
// The keys the map uses; Escape only ends a box zoom, so its default stays.
const KEYS = new Set(['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', '+', '=', '-', '_']);
// Where the canvas is: read on each press and resize rather than on every
// move, which would lay the page out again.
let origin = canvas.getBoundingClientRect();
const at = (e) => [e.clientX - origin.left, e.clientY - origin.top];

canvas.tabIndex = 0;
canvas.style.touchAction = 'none';
canvas.style.outline = 'none';
canvas.addEventListener('pointerdown', (e) => {
  canvas.focus({ preventScroll: true });
  origin = canvas.getBoundingClientRect();
  send('pointer_down', e.pointerId, e.pointerType, e.button, ...at(e), modifiers(e), time(e));
  // The map drags with the primary and secondary buttons.
  if (e.button === 0 || e.button === 2) canvas.setPointerCapture(e.pointerId);
});
// In a worker, the map draws as soon as a move arrives: pointerrawupdate
// comes when the browser has the move, pointermove only with the page's
// next frame.
const moveEvent = 'onpointerrawupdate' in HTMLElement.prototype ? 'pointerrawupdate' : 'pointermove';
canvas.addEventListener(moveEvent, (e) => send('pointer_move', e.pointerId, ...at(e), time(e)));
canvas.addEventListener('pointerup', (e) => send('pointer_up', e.pointerId, ...at(e), time(e)));
canvas.addEventListener('pointercancel', (e) => send('pointer_cancel', e.pointerId, time(e)));
// A wheel event as the recognizer takes it (WebMap::wheel's `kind`):
// [WHEEL_NOTCHES, notches] for a mouse wheel's detents, which it eases, or
// [WHEEL_PIXELS, pixels] for continuous scrolling (a trackpad, a precise
// wheel), which it applies at once. Positive scrolls down. Browsers don't
// say which device scrolled, so this reads it from how they report each:
// - Firefox reports wheels in lines, 3 a notch, and pages.
// - Safari reports a notch as 4.000244140625 px, and a faster turn as a
//   whole multiple of it (GL JS's test): one notch per multiple.
// - Chromium's legacy wheelDeltaY is 120 a detent, and -3 × deltaY for
//   precise scrolling.
// Untested on real hardware; a misread zooms the same way by a similar
// amount, with or without the easing.
const WHEEL_PIXELS = 0;
const WHEEL_NOTCHES = 1;
const SAFARI_NOTCH = 4.000244140625;
function wheelDelta(e) {
  if (e.deltaMode === WheelEvent.DOM_DELTA_LINE) return [WHEEL_NOTCHES, e.deltaY / 3];
  if (e.deltaMode === WheelEvent.DOM_DELTA_PAGE) return [WHEEL_NOTCHES, e.deltaY];
  if (e.deltaY !== 0 && e.deltaY % SAFARI_NOTCH === 0) return [WHEEL_NOTCHES, e.deltaY / SAFARI_NOTCH];
  const legacy = e.wheelDeltaY;
  if (typeof legacy === 'number' && legacy !== 0 && Math.abs(legacy + 3 * e.deltaY) >= 1 && legacy % 120 === 0) {
    return [WHEEL_NOTCHES, -legacy / 120];
  }
  return [WHEEL_PIXELS, e.deltaY];
}
canvas.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (e.deltaY !== 0) send('wheel', ...at(e), ...wheelDelta(e), modifiers(e), time(e));
}, { passive: false });
canvas.addEventListener('keydown', (e) => {
  if (KEYS.has(e.key)) e.preventDefault();
  if (KEYS.has(e.key) || e.key === 'Escape') send('key', e.key, modifiers(e), time(e));
});
canvas.addEventListener('blur', (e) => send('blur', time(e)));
canvas.addEventListener('contextmenu', (e) => e.preventDefault());

// A Shift-drag's box, which the worker reports as [x0, y0, x1, y1] while it
// is drawn, or null.
const box = $('box');
function showBox(corners) {
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
  map.postMessage({ resize: size });
}).observe(canvas);

// The camera in the hash, #zoom/lat/lon[/bearing/pitch], as a camera
// ([lat, lon, zoom, bearing, pitch]), or null.
function cameraFromHash(hash) {
  const parts = hash.slice(1).split('/').map(Number);
  if (parts.length < 3 || parts.length > 5 || parts.some((part) => !Number.isFinite(part))) return null;
  const [zoom, lat, lon, bearing = 0, pitch = 0] = parts;
  return [lat, lon, zoom, bearing, pitch];
}

function hashFromCamera([lat, lon, zoom, bearing, pitch]) {
  // Enough digits for a pixel at this zoom, as GL JS writes them.
  const digits = Math.max(0, Math.ceil(Math.log10(2 ** zoom * 512 / 360 / 0.5)));
  const round = (value, places) => String(Number(value.toFixed(places)));
  let hash = `#${round(zoom, 2)}/${round(lat, digits)}/${round(lon, digits)}`;
  if (Math.abs(bearing) >= 0.05 || Math.abs(pitch) >= 0.5) hash += `/${round(bearing, 1)}/${round(pitch, 0)}`;
  return hash;
}

// The hash follows the camera, written at most every 300 ms (browsers limit
// how often a page may replace its URL).
let writtenHash = location.hash;
let hashTimer = null;
let latestCamera = camera;
const needle = $('needle');
function showCamera(next) {
  latestCamera = next;
  const [, , , bearing, pitch] = next;
  needle.style.transform = `rotateX(${pitch}deg) rotateZ(${-bearing}deg)`;
  hashTimer ??= setTimeout(() => {
    hashTimer = null;
    writtenHash = hashFromCamera(latestCamera);
    history.replaceState(history.state, '', writtenHash);
  }, 300);
}
window.addEventListener('hashchange', () => {
  if (location.hash === writtenHash) return;
  const next = cameraFromHash(location.hash);
  if (next) map.postMessage({ jumpTo: next });
});

// Zoom and the compass.
$('zoom-in').addEventListener('click', (e) => send('key', '+', 0, time(e)));
$('zoom-out').addEventListener('click', (e) => send('key', '-', 0, time(e)));
$('compass').addEventListener('click', () => map.postMessage({ resetNorth: true }));

// The style button and its list.
let loading = true;
let styleName = '';
function showStyle() {
  $('style-name').textContent = current.key === null ? styleName || current.name : current.name;
  $('style-detail').textContent = fatal ? 'Stopped' : loading ? 'Loading…' : current.provider;
  document.documentElement.dataset.theme = current.dark ? 'dark' : 'light';
  for (const option of $('style-list').querySelectorAll('.style-option')) {
    option.setAttribute('aria-current', String(option.dataset.key === current.key));
  }
}
function pickStyle(next) {
  $('styles').hidePopover();
  if (next.url === current.url) return;
  current = next;
  loading = true;
  styleName = '';
  showStyle();
  showAttribution([]);
  map.postMessage({ style: next.url });
  const url = new URL(location.href);
  url.searchParams.set('style', next.key);
  url.hash = writtenHash;
  history.replaceState(history.state, '', url);
}
$('style-list').replaceChildren(...STYLES.map((option) => {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'style-option';
  button.dataset.key = option.key;
  button.innerHTML = '<span class="style-text"><span class="style-name"></span><span class="style-detail"></span></span>';
  button.querySelector('.style-name').textContent = option.name;
  button.querySelector('.style-detail').textContent = option.provider;
  button.addEventListener('click', () => pickStyle(option));
  const item = document.createElement('li');
  item.append(button);
  return item;
}));
showStyle();

// Attribution: the sources' HTML, kept to its text and its http(s) links.
const attribution = $('attribution');
const attributionToggle = $('attribution-toggle');
attributionToggle.addEventListener('click', () => {
  const open = !attribution.classList.contains('open');
  attribution.classList.toggle('open', open);
  attributionToggle.setAttribute('aria-expanded', String(open));
});
function sanitize(node, into) {
  for (const child of node.childNodes) {
    if (child.nodeType === Node.TEXT_NODE) {
      into.append(child.textContent);
    } else if (child.nodeName === 'A' && /^https?:\/\//.test(child.getAttribute('href') ?? '')) {
      const link = document.createElement('a');
      link.href = child.getAttribute('href');
      link.target = '_blank';
      link.rel = 'noopener';
      link.textContent = child.textContent;
      into.append(link);
    } else if (child.nodeType === Node.ELEMENT_NODE) {
      sanitize(child, into);
    }
  }
}
function showAttribution(entries) {
  const text = $('attribution-text');
  const all = entries.map((entry) => {
    const part = document.createElement('span');
    sanitize(new DOMParser().parseFromString(entry, 'text/html').body, part);
    return part;
  });
  // Each credit once: an entry is left out when a longer one, or an equal
  // one before it, contains its text.
  const texts = all.map((part) => part.textContent.trim());
  const repeated = (i) => texts.some((other, j) =>
    j !== i && other.includes(texts[i]) && (other.length > texts[i].length || j < i));
  const parts = all.filter((_, i) => texts[i] && !repeated(i));
  text.replaceChildren(...parts.flatMap((part, i) => (i ? [' ', part] : [part])));
  attribution.hidden = parts.length === 0;
}
