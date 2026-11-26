# mln-rs in the browser

A demo page that runs mln-rs compiled to `wasm32-unknown-unknown`, with Web
Workers in place of threads. The build uses no atomics or shared memory, so it
needs no COOP/COEP headers.

## Build and run

.config/mise/config.toml pins the tools: wasm-bindgen-cli, binaryen's
`wasm-opt`, Node and pnpm.

```sh
mise install
mise run web:build  # build into dist/
mise run web:dev    # Vite's dev server, from the sources
```

`web:build` builds the wasm modules into web/pkg, then Vite bundles the page
(web/, TypeScript) into dist/. The default `dev-opt` profile builds optimized
code with debug checks and keeps symbol names for debugging and profiling. Use
`--profile dev` for unoptimized debugging, or `--profile release` for the
size-optimized build GitHub Pages deploys.

`pnpm -C examples/web typecheck` checks the page's TypeScript, and
`pnpm -C examples/web test` checks how it tells a mouse wheel from a trackpad
(web/wheel.ts) against each browser's events. No one has tried that with real
mice and trackpads in each browser yet.

Serve dist/ over HTTP and open `index.html`. It takes these URL parameters
(web/main.ts):

| Parameter                                | Effect                                                                                               |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `style=<key or URL>`                     | a style from [examples/styles.json](../styles.json), or any style URL (default: OpenFreeMap Liberty) |
| `lat`, `lon`, `zoom`, `bearing`, `pitch` | the starting camera, unless the URL hash has one (`#zoom/lat/lon[/bearing/pitch]`)                   |
| `backend=webgpu` or `webgl2`             | the renderer (default: WebGPU where the browser has it)                                              |
| `workers=<n>`                            | the number of tile workers (default: up to 4)                                                        |
| `width`, `height`, `ratio`               | a fixed map size in CSS pixels, and pixel ratio                                                      |
| `compare`                                | overlay MapLibre GL JS's map of the same style, right of a draggable divider                         |
| `debug`                                  | the debug overlay: frame rate, frame time by phase, draw calls, memory, requests                     |
| `projection=flat`, `globe` or `handoff`  | Mercator, the sphere at every zoom, or the sphere until zoom 11–12, over the style's                 |
| `freeBearing`                            | free bearing on the globe                                                                            |
| `localIdeographFontFamily=false`         | download CJK glyphs rather than draw them with the browser's fonts                                   |
| `fallbackFontFaces=<json>`               | font files that every font stack falls back to, as a style's `font-faces` entry, unnamed             |
| `panic=map` or `panic=tile`              | make the map's or the first tile worker's wasm instance panic at startup, for the check              |

The page draws its own controls over the map: a style picker, a backend
indicator that switches between WebGPU and WebGL2 where both are available, zoom
buttons, a compass, attributions, and a panel that lists the gestures and has a
choice of `projection` and switches for `compare`, `debug` and `freeBearing`.
The style picker's Satellite 3D and Liberty 3D are VersaTiles' Satellite and
OpenFreeMap's Liberty with [Mapterhorn](https://mapterhorn.com)'s terrain added
once they load, and Liberty 3D with a hillshade of it and a sky (web/styles.ts).
The camera tilts up to 85°, past the map's default of 60°, to look across
terrain.

## How it runs

```
page (main thread, no wasm)
 ├─ map worker:    WebMap: a runtime, its map and a render session on the canvas
 │   └─ shaper module, loaded on demand
 ├─ tile worker 0: TileWorkerHost: parses, lays out and decodes tiles
 ├─ tile worker 1
 └─ ...
```

- The page compiles the wasm module once and sends it to each worker, which
  instantiates it with its own memory. The page transfers its canvas to the map
  worker as an OffscreenCanvas, and forwards pointer, wheel, keyboard and focus
  events to it. The map worker's gesture recognizer (mln_engine::gestures) turns
  them into camera moves.
- The map worker drives the map through mln_runtime, as any host does: a
  `Runtime` and its `Map`, and a `RenderSession` that presents to the canvas.
  The runtime's and the session's work runs in short tasks as it arrives, and
  each animation frame demands a frame when the map has something new to draw.
- Each worker runs its queues and pool tasks in a pump in place of threads, and
  finished fetches, tile replies and tasks are handled as they arrive, and
  retries and revalidations once they're due
  ([crates/mln_web_host](../../crates/mln_web_host)).
- The render session sends work to the tile workers as serialized messages over
  MessageChannels, which the runtime connects through a control port that the
  page gives the map worker for each tile worker. Large arrays, such as pixels
  and vertices, are transferred rather than copied.
- The module comes in two builds, and a browser downloads only one: WebGPU
  (mln_webgpu), or WebGL2 (mln_gl, the `webgl` feature without the default ones)
  for browsers without a working WebGPU.
- Shaping font faces (complex scripts, right-to-left text) needs HarfRust,
  skrifa and the glyph rasterizer, a large share of the code, so it lives in a
  second module, [crates/mln_web_shaper](../../crates/mln_web_shaper). The map
  worker loads it the first time a style declares `font-faces`. A panic there
  drops the affected text instead of stopping the map.
- Images decode with the browser's decoders, so the module carries none.
- CJK glyphs are drawn locally with an OffscreenCanvas instead of downloaded, as
  GL JS does with `localIdeographFontFamily`, and so are the glyphs of a style
  without glyph PBFs and those its PBFs lack: in the font stack's families, then
  the browser's `sans-serif` font.
- Resources go through `fetch` and the browser's HTTP cache. The build has no
  SQLite, so there's no cache database, no offline packs and no `mbtiles://`.
- A panic traps the wasm instance it happens in. If that's the map's or a tile
  worker's, the map stops and the page shows the panic message.
- When the browser loses the map's WebGPU device, as when its GPU process
  restarts, the page loads again at the same camera. A second loss within a
  minute counts as no working WebGPU, and the page loads again with WebGL2.

## Size

`mise run web:size` reports how much of the release module each crate and
function takes, and `mise run web:size main` how that changed from another
commit. `--open` also opens a zoomable treemap of crate, module and function in
the browser, colored by change in a comparison. The task builds the module as
`web:build --profile release` does, keeping the function names that
tools/wasm-size reads, and leaves out the name section, which the shipped module
doesn't have. `--module` picks mln_web_webgl or mln_web_shaper instead.

## Check

`mise run web:check` tests the browser build against the native renderer:

1. It loads views of the render test suite in headless Chromium, through
   Playwright, offline, with every fixture served from the suite's cache
   database.
2. The page reads each frame back from the canvas, in the task that drew it. A
   screenshot won't do, because headless Chromium composites WebGPU canvases as
   white.
3. The native render-test runner renders the same views and compares them with
   the page's frames.

It fails when a frame differs too much, when the page or a worker logs an error,
or when a view's tiles get no worker replies. Two views also drag and pinch the
map over a slow network first, and log the URLs it fetches again
(`crates/mln_engine/src/tests/fast_camera_test.rs` tests the gestures' tile
requests on a modeled network). It also runs mln_engine's conformance cases of
writes from JS arrays, and of writes while the instance's memory grows, on a GPU
of the map's backend, and checks that a panic stops the page, and that the GL JS
comparison lines up. The page's WebGPU frames come from SwiftShader, its WebGL2
ones from the first device ANGLE's Vulkan finds, and the native ones from
lavapipe, which match in most views. Where lavapipe isn't installed
(`/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`) and `VK_DRIVER_FILES` isn't set,
the native runner draws on the machine's default device, and the check says so.
check/check.mjs documents the options, and check/views.mjs lists the views.

Last, the check crashes Chromium's GPU process under a map drawn with WebGPU,
and fails unless the page loads again by itself and draws the same frame with
WebGPU, then crashes it again and fails unless the page loads again with WebGL2.

`--browser firefox` or `webkit` runs the check in Playwright's Firefox or WebKit
(`playwright-core install firefox webkit`), and `--channel chrome`, `msedge` or
`moz-firefox` in an installed browser, with `--executable` where Playwright
doesn't find it. Those draw on the machine's GPU, as Chromium does with `--gpu`.
Where the browser has no WebGPU, use `--backend webgl2`.

Before the first run, install the Chromium that the lockfile's playwright-core
pins, and on Linux keep lavapipe installed for the native runner, or point
`VK_DRIVER_FILES` at a Vulkan driver file:

```sh
pnpm -C examples/web install
pnpm -C examples/web exec playwright-core install --no-shell chromium
mise run web:check --filter real-world  # or no filter for every view
```

That install also deletes the browsers in Playwright's cache that no installed
Playwright version uses. `MLN_WEB_CHECK_LOG=1` prints the page's and workers'
consoles. CI runs the check with `--profile release`, once per backend.
