// The map's Web Worker: its own instance of the page's module, rendering
// into the page's canvas (an OffscreenCanvas), so that the map's work never
// runs on the page's thread. Its tile workers (worker.js) talk to it
// directly, each over a MessageChannel port the page connected: requests
// go as bytes, and replies come back as bytes and payloads, transferred.
//
// From the page: first {preload: {style, ratio, zoom}} as soon as the page
// starts, then {module, canvas, ports, width, height, ratio, style,
// fallbackFontFaces, camera, localIdeographFontFamily}, then {readPixels},
// {resize: [width, height]} in CSS pixels, and {input, args}: a call of
// WebMap's input methods (pointer_down, pointer_move, pointer_up,
// pointer_cancel, wheel, key, blur) whose last argument is the event's time
// as performance.timeOrigin + timeStamp. To the page: {status}, {loaded:
// {loadedMs, replies}} once the map is fully loaded, {pixels: {width,
// height, pixels}} for each readPixels, and {box} when a Shift-drag's box
// changes ([x0, y0, x1, y1], or null).

// Imported rather than a static import, so that the preloads below don't
// wait for it.
const bindings = import('./pkg/mln_web.js');

let start = null;
const early = []; // messages before the map exists
onmessage = ({ data }) => {
  if (data.preload) preloadStyle(data.preload.style, data.preload.ratio, data.preload.zoom);
  else if (data.module) start = run(data);
  else if (start) start.then((handle) => handle(data));
  else early.push(data);
};

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

async function run({ module, canvas, ports, width, height, ratio, style, fallbackFontFaces, camera, localIdeographFontFamily }) {
  const { default: init, WebMap, set_preloads } = await bindings;
  await init({ module_or_path: module });
  set_preloads(takePreload);

  // The map's work between frames (finished fetches, worker replies, queued
  // tasks) runs as soon as the map asks for it, in a task of its own, rather
  // than waiting for the next animation frame: each step of a load (a tile's
  // response, its parsing, the next request) would otherwise wait a frame.
  const ticks = new MessageChannel();
  let tickScheduled = false;
  const wake = () => {
    if (!tickScheduled) {
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
    (instance, bytes) => ports[instance].postMessage(bytes, [bytes.buffer]), wake, loadShaper,
    localIdeographFontFamily);

  ticks.port1.onmessage = () => {
    tickScheduled = false;
    if (map.tick(4)) wake();
  };
  ports.forEach((port, instance) => {
    port.onmessage = ({ data: replies }) => {
      for (const reply of replies) map.receive(instance, reply);
    };
  });

  if (fallbackFontFaces) map.set_fallback_font_faces(fallbackFontFaces);
  map.set_style_url(style);
  map.jump_to(...camera);
  const started = performance.now();

  let loaded = null;
  let readPixels = 0;
  let reported = 0;
  const report = (force) => {
    const now = performance.now();
    if (!force && now - reported < 250) return;
    reported = now;
    postMessage({
      status: `${ports.length} tile workers, ${map.received()} replies` +
        (loaded === null ? '' : `, loaded in ${Math.round(loaded.loadedMs)} ms`),
    });
  };
  function frame() {
    map.frame(8, performance.now());
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
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);

  // The page's events, on this worker's clock: the one frames run on.
  const INPUTS = new Set(['pointer_down', 'pointer_move', 'pointer_up', 'pointer_cancel', 'wheel', 'key', 'blur']);
  let box = null;
  const handle = (data) => {
    if (data.readPixels) {
      if (readPixels++ === 0) map.request_pixels();
    }
    if (data.resize) map.resize(...data.resize);
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
  };
  for (const data of early.splice(0)) handle(data);
  return handle;
}
