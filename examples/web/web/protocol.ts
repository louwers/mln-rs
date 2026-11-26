// The messages between the page (main.ts), the map's Web Worker
// (map-worker.ts) and the tile workers' Web Workers (tile-worker.ts).
//
// Each tile workers' Web Worker has a control port to the map's: the map's
// runtime connects each of its render sessions to it there, with a port of
// its own (TileWorkerConnect), and the Web Worker reports there
// (TileWorkerReport).

import type { Terrain } from "./styles.ts";

/** A projection type of the style spec. */
export type ProjectionType = "mercator" | "vertical-perspective" | "globe";

/** What the map draws with, each a build of the module. */
export type Backend = "webgpu" | "webgl2";

/** The module's bindings (pkg/mln_web.js). */
type Module = typeof import("./pkg/mln_web.js");
/** `names` of the bindings of either build of the module, and its start. */
export type Bindings<Names extends keyof Module> = Pick<Module, Names> & {
  default: (options: {
    module_or_path: WebAssembly.Module;
  }) => Promise<unknown>;
};

/** A camera: degrees, but for the zoom. */
export type Camera = [
  lat: number,
  lon: number,
  zoom: number,
  bearing: number,
  pitch: number,
];

/**
 * A call of one of WebMap's input methods, by the event's time as
 * performance.timeOrigin + timeStamp, which the map's Web Worker turns into
 * its own performance.now(). Positions are CSS pixels from the canvas's
 * top-left corner; modifiers are bits: Shift 1, Ctrl 2, Alt 4, Meta 8.
 */
export type Input =
  | {
      kind: "pointer_down";
      id: number;
      pointerType: string;
      button: number;
      x: number;
      y: number;
      modifiers: number;
      time: number;
    }
  | { kind: "pointer_move"; id: number; x: number; y: number; time: number }
  | { kind: "pointer_up"; id: number; x: number; y: number; time: number }
  | { kind: "pointer_cancel"; id: number; time: number }
  // `wheel` is WebMap::wheel's kind: WHEEL_NOTCHES or WHEEL_PIXELS.
  | {
      kind: "wheel";
      x: number;
      y: number;
      wheel: number;
      amount: number;
      modifiers: number;
      time: number;
    }
  | {
      kind: "key";
      key: string;
      modifiers: number;
      repeat: boolean;
      time: number;
    }
  // A modifier key went down or up: the modifiers held now.
  | { kind: "modifiers"; modifiers: number; time: number }
  | { kind: "blur"; time: number };

/** WebMap::wheel's `kind`: continuous scrolling, in pixels. */
export const WHEEL_PIXELS = 0;
/** WebMap::wheel's `kind`: a mouse wheel's detents, which the map eases. */
export const WHEEL_NOTCHES = 1;

/** Where a style loads from: a URL, or a style document's JSON text. */
export type StyleSource = { url: string } | { json: string };

/** What starts the map, once the module compiled. */
export interface Start {
  type: "start";
  module: WebAssembly.Module;
  canvas: OffscreenCanvas;
  /** One per tile workers' Web Worker: its control port. */
  tileWorkers: MessagePort[];
  /** CSS pixels. */
  width: number;
  height: number;
  ratio: number;
  style: StyleSource;
  /** The terrain the map adds to the style (styles.ts). */
  terrain: Terrain | undefined;
  /** JSON: font files that every font stack falls back to. */
  fallbackFontFaces: string | null;
  camera: Camera;
  /**
   * Whether the map draws CJK glyphs with the browser's fonts rather than
   * downloading them.
   */
  localIdeographs: boolean;
  /** A message to panic with once the map exists, for check/check.mjs. */
  panic: string | undefined;
}

/** From the page to the map's Web Worker. */
export type ToMap =
  | { type: "preload"; style: StyleSource; ratio: number; zoom: number }
  | Start
  | { type: "readPixels" }
  // Run mln_renderer's conformance cases of writes from JS arrays, and of
  // writes while the instance's memory grows, on a GPU of the map's backend
  // (WebMap::start_gpu_check), for check/check.mjs.
  | { type: "checkGpu" }
  | { type: "fetches" }
  | { type: "idle" }
  | { type: "nextFrame" }
  | { type: "resize"; width: number; height: number }
  | { type: "pixelRatio"; ratio: number }
  | { type: "style"; style: StyleSource; terrain: Terrain | undefined }
  | { type: "jumpTo"; camera: Camera }
  | { type: "resetNorth" }
  // The projection the map draws with in place of the style's
  // (WebMap::set_projection): a style spec projection type, or null for the
  // style's own.
  | { type: "projection"; projection: ProjectionType | null }
  // Whether drags and zooms on the globe turn the bearing
  // (WebMap::set_globe_free_bearing).
  | { type: "globeFreeBearing"; on: boolean }
  // Another map's camera, which this one follows.
  | { type: "follow"; camera: Camera }
  // Stop the camera's own motion: an ease, momentum, a wheel's eased zoom.
  | { type: "stop" }
  // The page's inputs of one task, such as the moves of a touch's fingers,
  // which the map applies before it draws again.
  | { type: "input"; inputs: Input[] }
  // Whether the page shows its debug overlay (debug.ts): while it does, the
  // map's Web Worker reports each frame that draws.
  | { type: "debug"; on: boolean };

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

/** A box-zoom drag's box, [x0, y0, x1, y1] in CSS pixels. */
export type Box = [number, number, number, number];

/** A frame that the map's Web Worker drew, for the debug overlay. */
export interface FrameSample {
  /** Milliseconds since the frame that drew before it. */
  interval: number;
  /**
   * Milliseconds of work since the frame before it, by phase, in PHASES's
   * order (WebMap::frame_timings).
   */
  phases: number[];
}

/** The phases of FrameSample's `phases`, in order. */
export const PHASES = ["render", "submit", "tasks"] as const;

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

/** From the map's Web Worker to the page. */
export type FromMap =
  | { type: "status"; text: string }
  // Once the map is fully loaded, and again once each later style is.
  | { type: "loaded"; loaded: Loaded }
  | { type: "pixels"; pixels: Pixels }
  // The GPU check passed; a failed one panics.
  | { type: "gpuChecked" }
  // Every URL the map's Web Worker fetched, and how many times.
  | { type: "fetches"; fetches: Record<string, number> }
  // The camera, once the map is fully loaded and has stopped moving.
  | { type: "idle"; camera: Camera }
  // The map's next animation frame has run, with the inputs before it.
  | { type: "nextFrame" }
  | { type: "box"; box: Box | null }
  // A gesture reached a notch: the page plays its haptic.
  | { type: "notch" }
  | { type: "camera"; camera: Camera }
  | { type: "attribution"; attribution: string[] }
  | { type: "styleName"; name: string }
  // While the page shows its debug overlay.
  | { type: "frame"; sample: FrameSample }
  | { type: "stats"; stats: DebugStats }
  // The map stopped: a panic, or it failed to start.
  | { type: "fatal"; message: string }
  // The map didn't start: the browser's API couldn't draw it.
  | { type: "unsupported"; message: string }
  // The map lost its device, as when the browser's GPU process restarts.
  | { type: "lost"; message: string };

/** From the page to a tile workers' Web Worker. */
export interface StartTileWorker {
  module: WebAssembly.Module;
  /** The control port, whose other end is the map's Web Worker. */
  control: MessagePort;
  /** A message to panic with once the instance starts, for check/check.mjs. */
  panic: string | undefined;
}

/**
 * From the map's runtime to a tile workers' Web Worker, over its control
 * port, for each render session (mln_web_host's tile_workers): a port to
 * serve the session's tile workers on.
 */
export interface TileWorkerConnect {
  module: WebAssembly.Module;
  port: MessagePort;
}

/**
 * From a tile workers' Web Worker to the map's, over its control port: how
 * many replies it sent in a batch, or that its instance panicked.
 */
export type TileWorkerReport = { replies: number } | { fatal: string };

/**
 * From a tile worker to the map's runtime, over a session's port: a batch of
 * replies, each its bytes and its payloads, or that its instance panicked.
 */
export type FromTileWorker = Uint8Array[][] | { fatal: string };

declare global {
  /** The message of the panic that aborted this instance (src/lib.rs). */
  var mlnPanic: string | undefined;
}
