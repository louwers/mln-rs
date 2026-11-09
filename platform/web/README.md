# mln-rs in the browser

The browser build: the libraries built for wasm32-unknown-unknown (no
atomics, no shared memory) and driven from JavaScript through wasm-bindgen.
It needs no COOP/COEP headers. The page compiles one wasm module, and Web
Workers instantiate it, each instance with its own memory. The page's
thread runs no wasm: it hands its canvas to the map's Web Worker and only
forwards to it.

- in the map's Web Worker (web/map-worker.js), `WebMap` (src/map.rs): the
  map, its renderer (wgpu's WebGPU backend, into the page's canvas as an
  OffscreenCanvas) and its file sources (fetch, src/fetch.rs). The
  instance has no threads, so it installs a `CooperativeScheduler`. Each
  animation frame runs the RunLoop, draws, and runs the scheduler's tasks
  until the frame's budget is spent. Between frames the RunLoop and the
  scheduler wake the worker (`set_waker`), which runs what arrived in a
  task of its own (`WebMap::tick`): a finished fetch or a worker's reply
  is handled at once, rather than at the next frame, so that a load's
  steps don't each wait for a frame.
- in each tile worker's Web Worker (web/worker.js), `TileWorker`
  (src/worker.rs): a `TileWorkerHost` on the instance's own
  `CooperativeScheduler`. The map's `SerializingTransport` gives each
  tile's worker to one of the Web Workers and sends every message between
  the two as bytes (mln_renderer's `tile::worker_codec`), over a
  MessageChannel that the page connects between the two Web Workers. So
  every vector and GeoJSON tile parses and lays out its buckets (symbols
  included) there, and every raster and raster-dem tile decodes its image
  there (the map gets back the pixels, and a DEM with the border it
  backfills from the neighbouring tiles). A reply's large arrays (pixels,
  vertices, indices) travel beside its bytes as their own buffers, which
  the Web Worker transfers, and which the map copies once, into the
  arrays its buckets keep.

## Build and run

`wasm-bindgen-cli` must be the version in Cargo.lock, and a release build
also runs binaryen's `wasm-opt` (version 130), which strips the names and
the other custom sections:

```sh
cargo install wasm-bindgen-cli --version 0.2.130
mise use -g aqua:WebAssembly/binaryen@130  # or binaryen's release, on the PATH
just profile=release web
python3 -m http.server -d platform/web/web
```

`just web` (the dev profile) skips wasm-opt and keeps the names, for
debugging and profiling.

Then open <http://localhost:8000/> in a browser with WebGPU. URL parameters
(web/main.js):

- `style=<url>` (default: OpenFreeMap Liberty), `lat`, `lon`, `zoom`,
  `bearing`, `pitch`;
- `workers=<n>`: Web Workers for the tiles (default: up to 4);
- `width`, `height`, `ratio`: a fixed canvas size in CSS pixels and pixel
  ratio.

## A panic stops a Web Worker

wasm32-unknown-unknown aborts on a panic, so the tile workers'
`catch_unwind`, which natively turns a panic while parsing (a malformed
vector tile, MVT or MLT) into an error for its tile, as C++ `catch (...)`
does, doesn't run in the browser. The panic aborts the Web Worker's wasm
instance instead: web/worker.js logs the instance as dead and drops its
requests, and every tile whose worker it hosts stays pending. TODO(port):
build with `panic=unwind` and wasm exception handling, or respawn the
instance and create its tiles' workers again.

## Check

`just profile=release web-check` (check/check.mjs, Node 24 with no
packages) loads views of the render suite in headless Chromium, offline
(Chromium resolves no host but the local server's), and compares each
with the native render-test runner's render of the same view. CI runs it
(the `web` job), the same way: install Playwright's Chromium once with
`npx playwright@1.61.1 install chromium` (the check uses that build), and
keep lavapipe for the native runner (`VK_DRIVER_FILES`).

- The views are render tests of the suite whose operations the page can
  follow (check.mjs, `VIEWS`): five real-world styles (vector tiles with
  fills, lines, icons and labels), raster tiles, raster-dem tiles
  (hillshade and color relief), and raster with GeoJSON. In the raster
  and raster-dem views only the raster tiles' workers can reply, so their
  replies show that those tiles decode in the Web Workers.
- A local server serves the page and every fixture from the suite's
  cache database (tools/render-test/testdata/metrics/cache-style.db), with
  the URLs that the runner's file source maps (`local://`, `mapbox://`,
  ...) rewritten to it. What the cache lacks gets no content, as in the
  runner.
- Once a view is fully loaded and its labels have faded in, the page
  reads its frame back from the GPU: headless Chromium composites WebGPU
  canvases as white, so a screenshot shows nothing.
  `WebMap::request_pixels` draws the frame again and copies the canvas
  texture into a buffer, which the event loop maps
  (`SurfaceBackend::take_readback`).
- The frames become the `expected.png` of render tests of the same views,
  which the native runner renders from the same cache database and
  compares with pixelmatch, as the render tests do.

It fails when more than 0.1% of a view's pixels differ (the combination
of raster-dem and vector at z1: 1%, where SwiftShader and lavapipe
antialias coastlines differently), when the page or a Web Worker logs an
error or throws (a worker that panics, a tile that fails to load), or
when a view gets no worker replies. The page renders on SwiftShader, which
Chromium bundles, and most views come out identical to lavapipe's;
leaving out one layer of labels changes 0.1% to 2% of the pixels.

Options: `--filter <regex>` picks views, `--workers <n>` sets the page's
Web Workers (default 2), `--codec <n>` renders natively through n codec
instances (`--tileWorkerCodec`), `--out <dir>` keeps the frames, the
native renders and their diffs, `--chromium <path>` uses another
Chromium. `MLN_WEB_CHECK_LOG=1` prints the consoles of the page and its
workers, and the fixtures the cache lacks.
