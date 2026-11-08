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

`just profile=release web-check` (check/check.mjs, Node with no packages)
serves web/, loads views of tools/liberty-compare/views.txt in headless
Chromium (`--enable-unsafe-webgpu`), and, once each map is fully loaded and
its labels have faded in, reads the frame back inside the page: headless
Chromium composites WebGPU canvases as white, so a screenshot shows
nothing. `WebMap::request_pixels` draws the frame again and copies the
canvas texture into a buffer, which the event loop maps
(`SurfaceBackend::take_readback`). The frames become the `expected.png` of
render tests of the same views, which the native render-test runner then
renders online (lavapipe) and compares with pixelmatch, as the render tests
do. It needs the network (tiles.openfreemap.org); CI doesn't run it.

It prints, for each view, the share of the pixels that pixelmatch counts
as different. Headless Chromium may pick SwiftShader for WebGPU while the
runner uses lavapipe, and then single antialiasing pixels along edges
differ; labels, shields and icons should be placed alike. `--codec <n>`
renders natively through n codec instances (`--tileWorkerCodec`),
`--workers <n>` sets the page's Web Workers.
