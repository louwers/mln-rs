// The messages between the page (main.ts), the map's Web Worker
// (map-worker.ts) and the tile workers' Web Workers (tile-worker.ts).

/** What the map draws with, each a build of the module. */
export type Backend = 'webgpu' | 'webgl2';

/** The module's bindings (pkg/mln_web.js). */
type Module = typeof import('./pkg/mln_web.js');
/** `names` of the bindings of either build of the module, and its start. */
export type Bindings<Names extends keyof Module> = Pick<Module, Names> & {
  default: (options: { module_or_path: WebAssembly.Module }) => Promise<unknown>;
};

/** A camera: degrees, but for the zoom. */
export type Camera = [lat: number, lon: number, zoom: number, bearing: number, pitch: number];

/**
 * A call of one of WebMap's input methods, by the event's time as
 * performance.timeOrigin + timeStamp, which the map's Web Worker turns into
 * its own performance.now(). Positions are CSS pixels from the canvas's
 * top-left corner; modifiers are bits: Shift 1, Ctrl 2, Alt 4, Meta 8.
 */
export type Input =
  | { kind: 'pointer_down'; id: number; pointerType: string; button: number; x: number; y: number; modifiers: number; time: number }
  | { kind: 'pointer_move'; id: number; x: number; y: number; time: number }
  | { kind: 'pointer_up'; id: number; x: number; y: number; time: number }
  | { kind: 'pointer_cancel'; id: number; time: number }
  // `wheel` is WebMap::wheel's kind: WHEEL_NOTCHES or WHEEL_PIXELS.
  | { kind: 'wheel'; x: number; y: number; wheel: number; amount: number; modifiers: number; time: number }
  | { kind: 'key'; key: string; modifiers: number; time: number }
  | { kind: 'blur'; time: number };

/** WebMap::wheel's `kind`: continuous scrolling, in pixels. */
export const WHEEL_PIXELS = 0;
/** WebMap::wheel's `kind`: a mouse wheel's detents, which the map eases. */
export const WHEEL_NOTCHES = 1;

/** What starts the map, once the module compiled. */
export interface Start {
  type: 'start';
  module: WebAssembly.Module;
  canvas: OffscreenCanvas;
  /** One per tile worker, connected to it. */
  ports: MessagePort[];
  /** CSS pixels. */
  width: number;
  height: number;
  ratio: number;
  style: string;
  /** JSON: font files that every font stack falls back to. */
  fallbackFontFaces: string | null;
  camera: Camera;
  localIdeographFontFamily: string | undefined;
  /** A message to panic with once the map exists, for check/check.mjs. */
  panic: string | undefined;
}

/** From the page to the map's Web Worker. */
export type ToMap =
  | { type: 'preload'; style: string; ratio: number; zoom: number }
  | Start
  | { type: 'readPixels' }
  | { type: 'fetches' }
  | { type: 'idle' }
  | { type: 'resize'; width: number; height: number }
  | { type: 'pixelRatio'; ratio: number }
  | { type: 'style'; url: string }
  | { type: 'jumpTo'; camera: Camera }
  | { type: 'resetNorth' }
  // Whether drags and zooms on the globe turn the bearing
  // (WebMap::set_globe_free_bearing).
  | { type: 'globeFreeBearing'; on: boolean }
  // Another map's camera, which this one follows.
  | { type: 'follow'; camera: Camera }
  // Stop the camera's own motion: an ease, momentum, a wheel's eased zoom.
  | { type: 'stop' }
  | { type: 'input'; input: Input }
  // Whether the page shows its debug overlay (debug.ts): while it does, the
  // map's Web Worker reports each frame and the features under `hover`.
  | { type: 'debug'; on: boolean }
  // The pointer over the canvas, in CSS pixels, or null once it left.
  | { type: 'hover'; point: [number, number] | null };

export interface Loaded {
  loadedMs: number;
  replies: number;
}

export interface Pixels {
  width: number;
  height: number;
  /** RGBA rows of the canvas's physical size. */
  pixels: Uint8Array;
}

/** A Shift-drag's box, [x0, y0, x1, y1] in CSS pixels. */
export type Box = [number, number, number, number];

/** One animation frame of the map's Web Worker, for the debug overlay. */
export interface FrameSample {
  /** Milliseconds since the frame before it. */
  interval: number;
  /** Milliseconds of each phase, in PHASES's order (WebMap::frame_timings). */
  phases: number[];
}

/** The phases of FrameSample's `phases`, in order. */
export const PHASES = ['update', 'render', 'tasks', 'between'] as const;

/** What the debug overlay shows of the map beside its frames. */
export interface DebugStats {
  /** Draw calls of the last frame drawn. */
  drawCalls: number;
  /** Bytes of the map's wasm instance's memory. */
  memory: number;
  /** Bytes of the live textures and buffers on the GPU. */
  gpuMemory: number;
  /** Requests the map's Web Worker is waiting on, and all it has made. */
  pending: number;
  requests: number;
  /** Milliseconds the current style took to load fully, once it has. */
  loadedMs: number | null;
}

/**
 * A feature under the pointer (WebMap::query_features). `type` is its
 * style layer's, and `geometry` its geometry's GeoJSON type.
 */
export interface DebugFeature {
  layer: string;
  type: string | null;
  source: string;
  sourceLayer: string;
  id: string | number | null;
  geometry: string;
  properties: Record<string, unknown>;
  state: Record<string, unknown>;
}

/** From the map's Web Worker to the page. */
export type FromMap =
  | { type: 'status'; text: string }
  // Once the map is fully loaded, and again once each later style is.
  | { type: 'loaded'; loaded: Loaded }
  | { type: 'pixels'; pixels: Pixels }
  // Every URL the map's Web Worker fetched, and how many times.
  | { type: 'fetches'; fetches: Record<string, number> }
  // The camera, once the map is fully loaded and has stopped moving.
  | { type: 'idle'; camera: Camera }
  | { type: 'box'; box: Box | null }
  | { type: 'camera'; camera: Camera }
  | { type: 'attribution'; attribution: string[] }
  | { type: 'styleName'; name: string }
  // While the page shows its debug overlay.
  | { type: 'frame'; sample: FrameSample }
  | { type: 'stats'; stats: DebugStats }
  // `at` is the pointer's [lat, lon], or null past the globe's edge.
  | { type: 'features'; at: [number, number] | null; features: DebugFeature[] }
  // The map stopped: a panic, or it failed to start.
  | { type: 'fatal'; message: string }
  // The map didn't start: the browser's API couldn't draw it.
  | { type: 'unsupported'; message: string };

/** From the page to a tile worker's Web Worker. */
export interface StartTileWorker {
  module: WebAssembly.Module;
  /** Connected to the map's Web Worker. */
  port: MessagePort;
  /** A message to panic with once the instance starts, for check/check.mjs. */
  panic: string | undefined;
}

/**
 * From a tile worker to the map's Web Worker, over its port: a batch of
 * replies, each its bytes and its payloads, or that its instance panicked.
 */
export type FromTileWorker = Uint8Array[][] | { fatal: string };

declare global {
  /** The message of the panic that aborted this instance (src/lib.rs). */
  var mlnPanic: string | undefined;
}
