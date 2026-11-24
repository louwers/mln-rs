// The style picker's previews (picker.ts), in a Web Worker of their own: an
// instance of the page's module with a map per preview (WebMap), each on its
// preview's canvas. Their work stays off the map's thread and the page's,
// and the page ends it, and frees its memory, by terminating the worker when
// the picker closes. The page gives the worker tile workers' Web Workers of
// its own, which the previews share.
//
// The page sends the start, then the rest of ToPreviews (protocol.ts). Every
// preview shows the camera the page sent last, from its next animation frame
// on.
//
// A panic in this instance or a tile worker's, or a browser whose API can't
// draw a preview, stops them all: the worker says `fatal`, with the message
// the panic hook left in `mlnPanic` (src/lib.rs), and nothing calls into the
// instance after that.
//
// The page names this worker `webgl2` where the maps draw with WebGL2: it
// then runs that build of the module (pkg/webgl). A browser gives a Web
// Worker only a few WebGL2 contexts (Chromium four), so there the maps all
// draw with the context of one canvas of the worker's own, a whole frame at
// a time, and each frame goes from it to its preview's canvas as a bitmap.
// With WebGPU, each map draws into its preview's canvas.

import type { WebMap } from "./pkg/mln_web.js";
import type {
  Bindings,
  Camera,
  FromPreviews,
  Preview,
  StartPreviews,
  TileWorkerReport,
  ToPreviews,
} from "./protocol.ts";

const webgl2 = self.name === "webgl2";

const bindings: Promise<
  Bindings<"WebMap" | "mlnWebStopPump" | "mlnWebConfigureTileWorkers">
> = webgl2 ? import("./pkg/webgl/mln_web.js") : import("./pkg/mln_web.js");

let stopped = false;
// Stops the instance's work between frames, once it has started.
let stopPump = () => {};

function fatal(error: unknown) {
  if (stopped) return;
  stopped = true;
  try {
    stopPump();
  } catch {
    // The instance panicked: nothing runs in it any more.
  }
  const message: FromPreviews = {
    type: "fatal",
    message: self.mlnPanic ?? String(error),
  };
  postMessage(message);
}

// Calls into the instance, unless the previews have stopped.
function guard(f: () => void) {
  if (stopped) return;
  try {
    f();
  } catch (error) {
    fatal(error);
  }
}

// What throws outside `guard`: a fetch's or an image decode's completion.
addEventListener("error", (event) => fatal(event.error ?? event.message));
addEventListener("unhandledrejection", (event) => fatal(event.reason));

// The previews' maps, each from when it starts.
const maps: WebMap[] = [];
// With WebGL2, the canvas that every map draws into, and what shows a
// frame taken from it in each preview's canvas. A preview's canvas is as
// large as the frames it shows.
const shared = webgl2 ? new OffscreenCanvas(1, 1) : null;
const shown: { canvas: OffscreenCanvas; show: ImageBitmapRenderingContext }[] =
  [];
let size: [number, number] = [1, 1];
let ratio = 1;
function fit(canvas: OffscreenCanvas) {
  canvas.width = Math.max(1, Math.floor(size[0] * ratio));
  canvas.height = Math.max(1, Math.floor(size[1] * ratio));
}
// The camera the page sent last, and whether a map has yet to show it.
let camera: Camera = [0, 0, 0, 0, 0];
let moved = false;
// The messages that wait until every map has started.
let early: ToPreviews[] | null = [];

onmessage = ({ data }: MessageEvent<ToPreviews>) => {
  if (data.type === "start") {
    run(data).catch(fatal);
  } else if (data.type === "camera") {
    camera = data.camera;
    moved = true;
  } else if (early) {
    early.push(data);
  } else {
    guard(() => handle(data));
  }
};

function handle(data: ToPreviews) {
  switch (data.type) {
    case "projection":
      for (const map of maps) map.set_projection(data.projection ?? undefined);
      break;
    case "resize":
      size = [data.width, data.height];
      for (const { canvas } of shown) fit(canvas);
      for (const map of maps) map.resize(data.width, data.height);
      break;
    case "pixelRatio":
      ratio = data.ratio;
      for (const { canvas } of shown) fit(canvas);
      for (const map of maps) map.set_pixel_ratio(data.ratio);
      break;
    case "start":
    case "camera":
      break;
  }
}

async function run(start: StartPreviews) {
  const {
    default: init,
    WebMap,
    mlnWebStopPump,
    mlnWebConfigureTileWorkers,
  } = await bindings;
  await init({ module_or_path: start.module });
  stopPump = mlnWebStopPump;

  // The maps' render sessions connect to the tile workers' Web Workers over
  // their control ports, which report their panics.
  for (const control of start.tileWorkers) {
    control.onmessage = ({ data }: MessageEvent<TileWorkerReport>) => {
      if ("fatal" in data) fatal(data.fatal);
    };
  }
  let spawned = 0;
  mlnWebConfigureTileWorkers(
    start.tileWorkers.length,
    () => start.tileWorkers[spawned++],
  );

  // A style's font faces are shaped by a second module, which the instance
  // loads the first time a style declares them.
  const loadShaper = async () => {
    const shaper = await import("./pkg/mln_web_shaper.js");
    await shaper.default();
    return shaper;
  };

  camera = start.camera;
  size = [start.width, start.height];
  ratio = start.ratio;
  const create = async ({ canvas, style, terrain }: Preview) => {
    const show = shared ? canvas.getContext("bitmaprenderer") : null;
    if (show) {
      fit(canvas);
      shown.push({ canvas, show });
    }
    // The map rendered frames: the shared canvas has the last one until
    // this task ends, or another map draws.
    let rendering: WebMap | undefined;
    const onFrame = () =>
      guard(() => {
        if (rendering && rendering.drain_frames() > 0 && shared && show)
          show.transferFromImageBitmap(shared.transferToImageBitmap());
      });
    const map = await WebMap.create(
      shared ?? canvas,
      start.width,
      start.height,
      start.ratio,
      loadShaper,
      start.fallbackFontFaces,
      start.localIdeographs,
      onFrame,
    );
    rendering = map;
    guard(() => {
      const terrainJson =
        terrain === undefined ? undefined : JSON.stringify(terrain);
      if ("url" in style) map.set_style_url(style.url, terrainJson);
      else map.set_style_json(style.json, terrainJson);
      map.set_projection(start.projection ?? undefined);
      map.jump_to(...camera);
      maps.push(map);
    });
  };

  // Each preview draws from when its map starts.
  function frame() {
    guard(() => {
      const now = performance.now();
      const jump = moved;
      moved = false;
      for (const map of maps) {
        if (jump) map.jump_to(...camera);
        map.frame(now);
      }
    });
    if (!stopped) requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);

  await Promise.all(start.previews.map(create));
  const waiting = early ?? [];
  early = null;
  guard(() => {
    for (const data of waiting) handle(data);
  });
}
