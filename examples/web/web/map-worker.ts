// The map's Web Worker: its own instance of the page's module, rendering
// into the page's canvas (an OffscreenCanvas), so that the map's work never
// runs on the page's thread. Its tile workers (worker.ts) talk to it
// directly, each over a MessageChannel port the page connected: requests
// go as bytes, and replies come back as bytes and payloads, transferred.
//
// The page sends a preload as soon as it starts, the start once the module
// compiled, then the rest of ToMap (protocol.ts); this worker answers with
// FromMap.
//
// A panic aborts the instance it happens in (wasm32-unknown-unknown can't
// unwind), and nothing may call into it after that. When this instance or a
// tile worker's panics, the map stops: no more frames, ticks or replies, and
// a fatal message goes to the page, with the message the panic hook left in
// `mlnPanic` (src/lib.rs) or a tile worker sent. `panic`, if given, is a
// message to panic with once the map exists (`panic_for_check`), for
// check/check.mjs. So is the count of every URL this worker fetched, which
// the page asks for with `fetches`.
//
// The page names this worker `webgl2` where the map draws with WebGL2: it
// then runs that build of the module (pkg/webgl). When the browser's API
// can't start the map, the worker says `unsupported` rather than `fatal`.

import type { WebMap } from './pkg/mln_web.js';
import type { Bindings, Box, Camera, FromMap, FromTileWorker, Input, Loaded, Start, ToMap } from './protocol.ts';

// Every URL this worker fetched, and how many times: the map's fetches look
// the global `fetch` up at each call, and so do the preloads below.
const fetches = new Map<string, number>();
const globalFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
  const url = input instanceof Request ? input.url : String(input);
  fetches.set(url, (fetches.get(url) ?? 0) + 1);
  return globalFetch(input, init);
};

const webgl2 = self.name === 'webgl2';

// Imported rather than a static import, so that the preloads below don't
// wait for it.
const bindings: Promise<Bindings<'WebMap' | 'set_preloads' | 'panic_for_check'>> = webgl2
  ? import('./pkg/webgl/mln_web.js')
  : import('./pkg/mln_web.js');

type Handle = (message: ToMap) => void;

// Frames without a camera change after which the map has stopped moving.
const IDLE_FRAMES = 10;

let start: Promise<Handle | undefined> | null = null;
const early: ToMap[] = []; // messages before the map exists
onmessage = ({ data }: MessageEvent<ToMap>) => {
  if (data.type === 'preload') preloadStyle(data.style, data.ratio, data.zoom);
  else if (data.type === 'fetches') post({ type: 'fetches', fetches: Object.fromEntries(fetches) });
  else if (data.type === 'start') start = run(data).catch((error: unknown) => fatal('map', error));
  else if (start) start.then((handle) => handle?.(data));
  else early.push(data);
};

const post = (message: FromMap, transfer: Transferable[] = []) => postMessage(message, transfer);

let stopped = false;

function fatal(where: string, error: unknown): undefined {
  if (stopped) return;
  stopped = true;
  post({ type: 'fatal', message: `${where}: ${self.mlnPanic ?? String(error)}` });
}

// Calls into the instance, unless the map has stopped.
function guard(f: () => void) {
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
const preloads = new Map<string, { response: Promise<Response>; expiry: ReturnType<typeof setTimeout> }>();

function takePreload(url: string): Promise<Response> | undefined {
  const preloaded = preloads.get(url);
  if (preloaded === undefined) return undefined;
  preloads.delete(url);
  clearTimeout(preloaded.expiry);
  return preloaded.response;
}

function preload(url: unknown): Promise<Response> | undefined {
  if (typeof url !== 'string' || !/^https?:\/\//.test(url) || preloads.has(url)) return undefined;
  const response = fetch(url);
  // Unclaimed, a failure is quiet; claimed, the map fetches again.
  response.catch(() => {});
  const expiry = setTimeout(() => preloads.delete(url), 60_000);
  preloads.set(url, { response, expiry });
  return response;
}

function preloadStyle(url: string, ratio: number, zoom: number) {
  // A clone to read: the map takes the original.
  preload(url)
    ?.then((response): Promise<unknown> | undefined => (response.ok ? response.clone().json() : undefined))
    .then((style) => {
      if (isRecord(style)) preloadParts(style, ratio, zoom);
    })
    .catch(() => {});
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function preloadParts(style: Record<string, unknown>, ratio: number, zoom: number) {
  for (const source of Object.values(isRecord(style.sources) ? style.sources : {})) {
    if (isRecord(source)) preload(source.url);
  }
  const sprites = typeof style.sprite === 'string' ? [style.sprite]
    : Array.isArray(style.sprite) ? style.sprite.map((sprite: unknown) => (isRecord(sprite) ? sprite.url : undefined)) : [];
  for (const sprite of sprites) {
    // mln_storage's `Resource::sprite_json` and `sprite_image`.
    if (typeof sprite !== 'string' || sprite.includes('#')) continue;
    const query = sprite.includes('?') ? sprite.indexOf('?') : sprite.length;
    const scale = ratio > 1 ? '@2x' : '';
    for (const extension of ['.json', '.png']) {
      preload(`${sprite.slice(0, query)}${scale}${extension}${sprite.slice(query)}`);
    }
  }
  const glyphs = style.glyphs;
  if (typeof glyphs === 'string') {
    for (const stack of firstFontStacks(style, zoom)) {
      // mln_storage's `Resource::glyphs`, for the first range.
      preload(glyphs.replaceAll('{fontstack}', percentEncode(stack)).replaceAll('{range}', '0-255'));
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
function firstFontStacks(style: Record<string, unknown>, zoom: number): Set<string> {
  const stacks = new Set<string>();
  for (const layer of Array.isArray(style.layers) ? style.layers : []) {
    if (!isRecord(layer)) continue;
    const layout = isRecord(layer.layout) ? layer.layout : {};
    const shown = layer.type === 'symbol' && layout.visibility !== 'none' &&
      (typeof layer.minzoom !== 'number' || zoom >= layer.minzoom) &&
      (typeof layer.maxzoom !== 'number' || zoom < layer.maxzoom);
    if (!shown || layout['text-field'] === undefined || layout['text-field'] === '') continue;
    const font = layout['text-font'];
    const fonts: unknown = font === undefined ? DEFAULT_FONT : Array.isArray(font) && font[0] === 'literal' ? font[1] : font;
    if (!Array.isArray(fonts) || fonts.length === 0) continue;
    const names = fonts.filter((name: unknown) => typeof name === 'string');
    if (names.length === fonts.length && !STRING_EXPRESSIONS.has(names[0] ?? '')) stacks.add(names.join(','));
  }
  return stacks;
}

// mln_util's `percent_encode`: every UTF-8 byte but the unreserved ones, as
// lowercase hex.
function percentEncode(text: string): string {
  let encoded = '';
  for (const byte of new TextEncoder().encode(text)) {
    const char = String.fromCharCode(byte);
    encoded += /[A-Za-z0-9\-_.~]/.test(char) ? char : `%${byte.toString(16).padStart(2, '0')}`;
  }
  return encoded;
}

async function run(
  { module, canvas, ports, width, height, ratio, style, fallbackFontFaces, camera, localIdeographFontFamily, panic }: Start,
): Promise<Handle> {
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
  const toTileWorker = (instance: number, bytes: Uint8Array) => {
    if (!stopped) ports[instance]?.postMessage(bytes, [bytes.buffer]);
  };
  let map: WebMap;
  try {
    map = await WebMap.create(canvas, width, height, ratio, ports.length, toTileWorker, wake, loadShaper,
      localIdeographFontFamily);
  } catch (error) {
    if (self.mlnPanic !== undefined) throw error;
    stopped = true;
    post({ type: 'unsupported', message: error instanceof Error ? error.message : String(error) });
    return () => {};
  }

  ticks.port1.onmessage = () => {
    tickScheduled = false;
    guard(() => {
      if (map.tick(4)) wake();
    });
  };
  ports.forEach((port, instance) => {
    port.onmessage = ({ data }: MessageEvent<FromTileWorker>) => {
      if ('fatal' in data) fatal(`tile worker ${instance}`, data.fatal);
      else guard(() => data.forEach((reply) => map.receive(instance, reply)));
    };
  });

  if (panic) panic_for_check(panic);

  if (fallbackFontFaces) map.set_fallback_font_faces(fallbackFontFaces);
  map.set_style_url(style);
  map.jump_to(...camera);

  let started = performance.now();
  let loaded: Loaded | null = null;
  let readPixels = 0;
  let idle = 0; // `idle` requests to answer
  let stillFrames = 0;
  let reported = 0;
  let attribution = '';
  let styleName = '';
  const report = (force: boolean) => {
    const now = performance.now();
    if (!force && now - reported < 250) return;
    reported = now;
    post({
      type: 'status',
      text: `${ports.length} tile workers, ${map.received()} replies` + (webgl2 ? ', WebGL2' : '') +
        (loaded === null ? '' : `, loaded in ${Math.round(loaded.loadedMs)} ms`),
    });
    // A source's attribution can come later than its style, with its TileJSON.
    const next = map.attributions();
    if (next.join('\n') !== attribution) {
      attribution = next.join('\n');
      post({ type: 'attribution', attribution: next });
    }
    const name = map.style_name();
    if (name !== styleName) {
      styleName = name;
      post({ type: 'styleName', name });
    }
  };
  function frame() {
    guard(drawFrame);
    if (!stopped) requestAnimationFrame(frame);
  }
  let shownCamera = '';
  function drawFrame() {
    map.frame(8, performance.now());
    const [lat = 0, lon = 0, zoom = 0, bearing = 0, pitch = 0] = map.camera();
    const next = [lat, lon, zoom, bearing, pitch].join();
    const camera: Camera = [lat, lon, zoom, bearing, pitch];
    if (next !== shownCamera) {
      shownCamera = next;
      stillFrames = 0;
      post({ type: 'camera', camera });
    } else {
      stillFrames++;
    }
    // Stopped: the camera hasn't moved for a few frames, past any ease the
    // last gesture started.
    if (idle > 0 && stillFrames >= IDLE_FRAMES && map.is_fully_loaded()) {
      for (; idle > 0; idle--) post({ type: 'idle', camera });
    }
    if (loaded === null && map.is_fully_loaded()) {
      loaded = { loadedMs: performance.now() - started, replies: map.received() };
      post({ type: 'loaded', loaded });
      report(true);
    }
    if (readPixels > 0) {
      const pixels = webgl2 ? canvasPixels(canvas) : map.take_pixels();
      if (pixels !== undefined) {
        readPixels--;
        post({ type: 'pixels', pixels: { width: canvas.width, height: canvas.height, pixels } }, [pixels.buffer]);
        if (readPixels > 0) map.request_pixels();
      }
    }
    report(false);
  }
  requestAnimationFrame(frame);

  let box: Box | null = null;
  const handle = (data: ToMap) => guard(() => {
    switch (data.type) {
      case 'readPixels':
        if (readPixels++ === 0) map.request_pixels();
        break;
      case 'idle':
        idle++;
        break;
      case 'resize':
        map.resize(data.width, data.height);
        break;
      case 'pixelRatio':
        map.set_pixel_ratio(data.ratio);
        break;
      case 'style':
        map.set_style_url(data.url);
        started = performance.now();
        loaded = null;
        // The page clears both for the new style, so they go again even
        // when the new style's are the same.
        attribution = '';
        styleName = '';
        report(true);
        break;
      case 'jumpTo':
        map.jump_to(...data.camera);
        break;
      case 'resetNorth':
        map.reset_north();
        break;
      case 'follow':
        map.follow(...data.camera);
        break;
      case 'stop': {
        const [lat = 0, lon = 0, zoom = 0, bearing = 0, pitch = 0] = map.camera();
        map.follow(lat, lon, zoom, bearing, pitch);
        break;
      }
      case 'input': {
        applyInput(map, data.input);
        const [x0, y0, x1, y1] = map.box_zoom() ?? [];
        const next: Box | null = x0 === undefined || y0 === undefined || x1 === undefined || y1 === undefined
          ? null
          : [x0, y0, x1, y1];
        if (String(next) !== String(box)) {
          box = next;
          post({ type: 'box', box });
        }
        break;
      }
      case 'preload':
      case 'start':
      case 'fetches':
        break;
    }
  });
  for (const data of early.splice(0)) handle(data);
  return handle;
}

// The frame just drawn, in the task that drew it: a WebGL2 canvas can't be
// copied from on the GPU, and keeps its frame only until it is shown. The
// rows are RGBA, not premultiplied.
function canvasPixels(canvas: OffscreenCanvas): Uint8Array | undefined {
  const copy = new OffscreenCanvas(canvas.width, canvas.height).getContext('2d');
  if (copy === null) return undefined;
  copy.drawImage(canvas, 0, 0);
  return new Uint8Array(copy.getImageData(0, 0, canvas.width, canvas.height).data.buffer);
}

// The page's events, on this worker's clock: the one frames run on.
function applyInput(map: WebMap, input: Input) {
  const time = input.time - performance.timeOrigin;
  switch (input.kind) {
    case 'pointer_down':
      map.pointer_down(input.id, input.pointerType, input.button, input.x, input.y, input.modifiers, time);
      break;
    case 'pointer_move':
      map.pointer_move(input.id, input.x, input.y, time);
      break;
    case 'pointer_up':
      map.pointer_up(input.id, input.x, input.y, time);
      break;
    case 'pointer_cancel':
      map.pointer_cancel(input.id, time);
      break;
    case 'wheel':
      map.wheel(input.x, input.y, input.wheel, input.amount, input.modifiers, time);
      break;
    case 'key':
      map.key(input.key, input.modifiers, time);
      break;
    case 'blur':
      map.blur(time);
      break;
  }
}
