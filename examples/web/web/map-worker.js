// The map's Web Worker: its own instance of the page's module, rendering
// into the page's canvas (an OffscreenCanvas), so that the map's work never
// runs on the page's thread. Its tile workers (worker.js) talk to it
// directly, each over a MessageChannel port the page connected: requests
// go as bytes, and replies come back as bytes and payloads, transferred.
//
// From the page: first {preload: {style, ratio, zoom}} as soon as the page
// starts, then {module, canvas, ports, width, height, ratio, style,
// fallbackFontFaces, camera, localIdeographFontFamily, panic}, then
// {readPixels}, {resize: [width, height]} in CSS pixels, {style: url},
// {jumpTo: camera}, {resetNorth}, and {input, args}: a call of WebMap's
// input methods (pointer_down, pointer_move, pointer_up, pointer_cancel,
// wheel, key, blur) whose last argument is the event's time as
// performance.timeOrigin + timeStamp. A camera is [lat, lon, zoom, bearing,
// pitch]. To the page: {status}, {loaded: {loadedMs, replies}} once the map
// is fully loaded, {pixels: {width, height, pixels}} for each readPixels,
// {box} when a Shift-drag's box changes ([x0, y0, x1, y1], or null),
// {camera} when the camera moves, {attribution: [html]} when the sources'
// attributions change, {styleName} when the style's name does, and {fatal}
// once the map stops.
//
// A panic aborts the instance it happens in (wasm32-unknown-unknown can't
// unwind), and nothing may call into it after that. When this instance or a
// tile worker's panics, the map stops: no more frames, ticks or replies, and
// {fatal: message} goes to the page, with the message the panic hook left
// in `mlnPanic` (src/lib.rs) or a tile worker sent. `panic`, if given, is a
// message to panic with once the map exists (`panic_for_check`), for
// check/check.mjs.

// Imported rather than a static import, so that the preloads below don't
// wait for it.
const bindings = import('./pkg/mln_web.js');

let start = null;
const early = []; // messages before the map exists
onmessage = ({ data }) => {
  if (data.preload) preloadStyle(data.preload.style, data.preload.ratio, data.preload.zoom);
  else if (data.module) start = run(data).catch((error) => fatal('map', error));
  else if (start) start.then((handle) => handle?.(data));
  else early.push(data);
};

let stopped = false;

function fatal(where, error) {
  if (stopped) return;
  stopped = true;
  postMessage({ fatal: `${where}: ${self.mlnPanic ?? String(error)}` });
}

// Calls into the instance, unless the map has stopped.
function guard(f) {
  if (stopped) return;
  try {
    f();
  } catch (error) {
    fatal('map', error);
  }
}

// What throws outside `guard`: a fetch's or an image decode's completion.
addEventListener('error', (event) => fatal('map', event.error ?? event.message));
addEventListener('unhandledrejection', (event) => fatal('map', event.reason));

// What the map fetches first, fetched while the module downloads and
// compiles: the style, then its sources' TileJSON, its sprite and its
// labels' first glyphs, at the URLs the map asks for them by. The map's
// fetches take these responses (mln_storage's `set_fetch_preloads`), each
// once, when they ask for the same URL; one never asked for is dropped
// after a minute. Only absolute http(s) URLs: the map's file source maps
// the others first.
const preloads = new Map();

function takePreload(url) {
  const preloaded = preloads.get(url);
  if (preloaded === undefined) return undefined;
  preloads.delete(url);
  clearTimeout(preloaded.expiry);
  return preloaded.response;
}

function preload(url) {
  if (typeof url !== 'string' || !/^https?:\/\//.test(url) || preloads.has(url)) return undefined;
  const response = fetch(url);
  // Unclaimed, a failure is quiet; claimed, the map fetches again.
  response.catch(() => {});
  const expiry = setTimeout(() => preloads.delete(url), 60_000);
  preloads.set(url, { response, expiry });
  return response;
}

function preloadStyle(url, ratio, zoom) {
  // A clone to read: the map takes the original.
  preload(url)
    ?.then((response) => (response.ok ? response.clone().json() : undefined))
    .then((style) => style && preloadParts(style, ratio, zoom))
    .catch(() => {});
}

function preloadParts(style, ratio, zoom) {
  for (const source of Object.values(style.sources ?? {})) preload(source?.url);
  const sprites = typeof style.sprite === 'string' ? [style.sprite]
    : Array.isArray(style.sprite) ? style.sprite.map((sprite) => sprite?.url) : [];
  for (const sprite of sprites) {
    // mln_storage's `Resource::sprite_json` and `sprite_image`.
    if (typeof sprite !== 'string' || sprite.includes('#')) continue;
    const query = sprite.includes('?') ? sprite.indexOf('?') : sprite.length;
    const scale = ratio > 1 ? '@2x' : '';
    for (const extension of ['.json', '.png']) {
      preload(`${sprite.slice(0, query)}${scale}${extension}${sprite.slice(query)}`);
    }
  }
  if (typeof style.glyphs === 'string') {
    for (const stack of firstFontStacks(style, zoom)) {
      // mln_storage's `Resource::glyphs`, for the first range.
      preload(style.glyphs.replaceAll('{fontstack}', percentEncode(stack)).replaceAll('{range}', '0-255'));
    }
  }
}

// The style spec's default `text-font`.
const DEFAULT_FONT = ['Open Sans Regular', 'Arial Unicode MS Regular'];
// Expressions whose arguments are all strings, unlike a font stack's.
const STRING_EXPRESSIONS = new Set(['get', 'has', 'var', 'feature-state', 'global-state', 'properties']);

// The font stacks, joined, of the symbol layers with text shown at `zoom`
// whose `text-font` is a constant: the glyphs their tiles' labels ask for
// first.
function firstFontStacks(style, zoom) {
  const stacks = new Set();
  for (const layer of Array.isArray(style.layers) ? style.layers : []) {
    const layout = layer?.layout ?? {};
    const shown = layer?.type === 'symbol' && layout.visibility !== 'none' &&
      (typeof layer.minzoom !== 'number' || zoom >= layer.minzoom) &&
      (typeof layer.maxzoom !== 'number' || zoom < layer.maxzoom);
    if (!shown || layout['text-field'] === undefined || layout['text-field'] === '') continue;
    const font = layout['text-font'];
    const fonts = font === undefined ? DEFAULT_FONT : Array.isArray(font) && font[0] === 'literal' ? font[1] : font;
    if (Array.isArray(fonts) && fonts.length > 0 && !STRING_EXPRESSIONS.has(fonts[0]) &&
      fonts.every((name) => typeof name === 'string')) {
      stacks.add(fonts.join(','));
    }
  }
  return stacks;
}

// mln_util's `percent_encode`: every UTF-8 byte but the unreserved ones, as
// lowercase hex.
function percentEncode(text) {
  let encoded = '';
  for (const byte of new TextEncoder().encode(text)) {
    const char = String.fromCharCode(byte);
    encoded += /[A-Za-z0-9\-_.~]/.test(char) ? char : `%${byte.toString(16).padStart(2, '0')}`;
  }
  return encoded;
}

async function run({ module, canvas, ports, width, height, ratio, style, fallbackFontFaces, camera, localIdeographFontFamily, panic }) {
  const { default: init, WebMap, set_preloads, panic_for_check } = await bindings;
  await init({ module_or_path: module });
  set_preloads(takePreload);

  // The map's work between frames (finished fetches, worker replies, queued
  // tasks) runs as soon as the map asks for it, in a task of its own, rather
  // than waiting for the next animation frame: each step of a load (a tile's
  // response, its parsing, the next request) would otherwise wait a frame.
  const ticks = new MessageChannel();
  let tickScheduled = false;
  const wake = () => {
    if (!tickScheduled && !stopped) {
      tickScheduled = true;
      ticks.port2.postMessage(null);
    }
  };
  // A style's font faces are shaped by a second module, which only this
  // worker loads, the first time a style declares them.
  const loadShaper = async () => {
    const shaper = await import('./pkg/mln_web_shaper.js');
    await shaper.default();
    return shaper;
  };
  const map = await WebMap.create(canvas, width, height, ratio, ports.length,
    (instance, bytes) => {
      if (!stopped) ports[instance].postMessage(bytes, [bytes.buffer]);
    }, wake, loadShaper,
    localIdeographFontFamily);

  ticks.port1.onmessage = () => {
    tickScheduled = false;
    guard(() => {
      if (map.tick(4)) wake();
    });
  };
  ports.forEach((port, instance) => {
    port.onmessage = ({ data }) => {
      if (data.fatal !== undefined) fatal(`tile worker ${instance}`, data.fatal);
      else guard(() => data.forEach((reply) => map.receive(instance, reply)));
    };
  });

  if (panic) panic_for_check(panic);

  if (fallbackFontFaces) map.set_fallback_font_faces(fallbackFontFaces);
  map.set_style_url(style);
  map.jump_to(...camera);

  let started = performance.now();
  let loaded = null;
  let readPixels = 0;
  let reported = 0;
  let attribution = '';
  let styleName = '';
  const report = (force) => {
    const now = performance.now();
    if (!force && now - reported < 250) return;
    reported = now;
    postMessage({
      status: `${ports.length} tile workers, ${map.received()} replies` +
        (loaded === null ? '' : `, loaded in ${Math.round(loaded.loadedMs)} ms`),
    });
    // A source's attribution can come later than its style, with its TileJSON.
    const next = map.attributions();
    if (next.join('\n') !== attribution) {
      attribution = next.join('\n');
      postMessage({ attribution: next });
    }
    const name = map.style_name();
    if (name !== styleName) {
      styleName = name;
      postMessage({ styleName });
    }
  };
  function frame() {
    guard(drawFrame);
    if (!stopped) requestAnimationFrame(frame);
  }
  let shownCamera = '';
  function drawFrame() {
    map.frame(8, performance.now());
    const next = map.camera();
    if (next.join() !== shownCamera) {
      shownCamera = next.join();
      postMessage({ camera: Array.from(next) });
    }
    if (loaded === null && map.is_fully_loaded()) {
      loaded = { loadedMs: performance.now() - started, replies: map.received() };
      postMessage({ loaded });
      report(true);
    }
    if (readPixels > 0) {
      const pixels = map.take_pixels();
      if (pixels !== undefined) {
        readPixels--;
        postMessage({ pixels: { width: canvas.width, height: canvas.height, pixels } }, [pixels.buffer]);
        if (readPixels > 0) map.request_pixels();
      }
    }
    report(false);
  }
  requestAnimationFrame(frame);

  // The page's events, on this worker's clock: the one frames run on.
  const INPUTS = new Set(['pointer_down', 'pointer_move', 'pointer_up', 'pointer_cancel', 'wheel', 'key', 'blur']);
  let box = null;
  const handle = (data) => guard(() => {
    if (data.readPixels) {
      if (readPixels++ === 0) map.request_pixels();
    }
    if (data.resize) map.resize(...data.resize);
    if (data.style) {
      map.set_style_url(data.style);
      started = performance.now();
      loaded = null;
      report(true);
    }
    if (data.jumpTo) map.jump_to(...data.jumpTo);
    if (data.resetNorth) map.reset_north();
    if (INPUTS.has(data.input)) {
      const args = data.args.slice();
      args[args.length - 1] -= performance.timeOrigin;
      map[data.input](...args);
      const next = map.box_zoom() ?? null;
      if (String(next) !== String(box)) {
        box = next && Array.from(next);
        postMessage({ box });
      }
    }
  });
  for (const data of early.splice(0)) handle(data);
  return handle;
}
