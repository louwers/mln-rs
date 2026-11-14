# mln-rs in the browser

A demo page that runs mln-rs compiled to `wasm32-unknown-unknown`, with Web
Workers in place of threads. The build uses no atomics or shared memory, so it
needs no COOP/COEP headers.

## Build and run

mise.toml pins the tools: wasm-bindgen-cli, binaryen's `wasm-opt`, Node and
pnpm.

```sh
mise install
mise run web:build --profile release  # into dist/, as GitHub Pages deploys it
mise run web:dev                      # Vite's dev server, from the sources
```

`web:build` builds the wasm modules into web/pkg, then Vite bundles the page
(web/, TypeScript) into dist/. The default dev profile skips `wasm-opt` and
keeps symbol names, for debugging and profiling.
`pnpm -C examples/web typecheck` checks the page's TypeScript, and
`pnpm -C examples/web test` checks how it tells a mouse wheel from a trackpad
(web/wheel.ts) against each browser's events. No one has tried that with real
mice and trackpads in each browser yet.

Serve dist/ over HTTP and open `index.html`. It takes these URL parameters
(web/main.ts):

| Parameter                                 | Effect                                                                                   |
| ----------------------------------------- | ---------------------------------------------------------------------------------------- |
| `style=<key or URL>`                      | a style from web/styles.ts, or any style URL (default: OpenFreeMap Liberty)              |
| `lat`, `lon`, `zoom`, `bearing`, `pitch`  | the starting camera, unless the URL hash has one (`#zoom/lat/lon[/bearing/pitch]`)       |
| `backend=webgpu` or `webgl2`              | the renderer (default: WebGPU where the browser has it)                                  |
| `workers=<n>`                             | the number of tile workers (default: up to 4)                                            |
| `width`, `height`, `ratio`                | a fixed map size in CSS pixels, and pixel ratio                                          |
| `compare`                                 | overlay MapLibre GL JS's map of the same style, right of a draggable divider             |
| `debug`                                   | the debug overlay: frame rate, frame time by phase, draw calls, memory, requests         |
| `freeBearing`                             | free bearing on the globe                                                                |
| `localIdeographFontFamily=<CSS families>` | the font for CJK glyphs (default: `sans-serif`); empty or `false` downloads them         |
| `fallbackFontFaces=<json>`                | font files that every font stack falls back to, as a style's `font-faces` entry, unnamed |
| `panic=map` or `panic=tile`               | make the map's or the first tile worker's wasm instance panic at startup, for the check  |

The page draws its own controls over the map: a style picker, zoom buttons, a
compass, attributions, and a panel that lists the gestures and has switches for
`compare`, `debug` and `freeBearing`.

## How it runs

```
page (main thread, no wasm)
 ├─ map worker:    WebMap: style, camera, renderer, file source
 │   └─ shaper module, loaded on demand
 ├─ tile worker 0: TileWorkerHost: parses, lays out and decodes tiles
 ├─ tile worker 1
 └─ ...
```

- The page compiles the wasm module once and sends it to each worker, which
  instantiates it with its own memory. The page transfers its canvas to the map
  worker as an OffscreenCanvas, and forwards pointer, wheel, keyboard and focus
  events to it. The map worker's gesture recognizer (mln_gestures) turns them
  into camera moves.
- Each worker runs a `CooperativeScheduler` in place of threads. The map worker
  runs its loop, draws and runs tasks on each animation frame. Between frames,
  finished fetches and tile replies are handled as they arrive
  ([crates/mln_web_host](../../crates/mln_web_host)).
- The map sends work to the tile workers as serialized messages over
  MessageChannels. Large arrays, such as pixels and vertices, are transferred
  rather than copied.
- The module comes in two builds, and a browser downloads only one: WebGPU
  (mln_webgpu), or WebGL2 (mln_gl, the `webgl` feature) for browsers without a
  working WebGPU.
- Shaping font faces (complex scripts, right-to-left text) needs HarfRust,
  skrifa and the FreeType port, a large share of the code, so it lives in a
  second module, [crates/mln_web_shaper](../../crates/mln_web_shaper). The map
  worker loads it the first time a style declares `font-faces`. A panic there
  drops the affected text instead of stopping the map.
- Images decode with the browser's decoders, so the module carries none.
- CJK glyphs are drawn locally with an OffscreenCanvas instead of downloaded, as
  GL JS does with `localIdeographFontFamily`.
- Resources go through `fetch` and the browser's HTTP cache. The build has no
  SQLite, so there's no cache database, no offline packs and no `mbtiles://`.
- A panic traps the wasm instance it happens in. If that's the map's or a tile
  worker's, the map stops and the page shows the panic message.

## Check

`mise run web:check` tests the browser build against the native renderer:

1. It loads views of the render test suite in headless Chromium, through
   Playwright, offline, with every fixture served from the suite's cache
   database.
2. The page reads each frame back from the GPU. A screenshot won't do, because
   headless Chromium composites WebGPU canvases as white.
3. The native render-test runner renders the same views and compares them with
   the page's frames.

It fails when a frame differs too much, when the page or a worker logs an error,
or when a view's tiles get no worker replies. Two views also drag and pinch the
map over a slow network first, and fail if that makes the map fetch URLs again.
It also checks that a panic stops the page, and that the GL JS comparison lines
up. The page's frames come from SwiftShader and the native ones from lavapipe,
which match in most views. check/check.mjs documents the options, and
check/views.mjs lists the views.

Before the first run, install the Chromium that the lockfile's playwright-core
pins, and keep lavapipe installed for the native runner (`VK_DRIVER_FILES`):

```sh
pnpm -C examples/web install
pnpm -C examples/web exec playwright-core install --no-shell chromium
mise run web:check --filter real-world  # or no filter for every view
```

That install also deletes the browsers in Playwright's cache that no installed
Playwright version uses. `MLN_WEB_CHECK_LOG=1` prints the page's and workers'
consoles. CI runs the check with `--profile release`, once per backend.
