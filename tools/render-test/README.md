# mln-rs-render-test

Ports MapLibre Native's render test runner (`render-test/`), which runs the
shared render and query tests.

`cargo test -p mln-rs-render-test --test integration render_tests::` runs them
twice, in one worker process per CPU (`MLN_RENDER_TEST_JOBS=<n>` sets the
number; 1 runs them in the test's own process): as they are, and with the tile
workers behind the worker codec of the browser build (`--tileWorkerCodec=2`). It
writes the actual and diff images, and HTML reports that show the failed and
errored tests, to `render_tests/` and `render_tests_through_the_codec/` in
`target/tmp/`, or in the directory that `MLN_TEST_OUTPUT` names.
`cargo run -p mln-rs-render-test -- --help` lists the runner's options, such as
`--filter`, `--jobs` and `--tileWorkerCodec`.

The tests render on WebGPU, or on the backend that `MLN_RENDER_BACKEND` names:
`vulkan`, `d3d12` (Windows; DXC's `dxcompiler.dll` on the DLL search path),
`gl`, `gl-desktop`, or `metal` (macOS). Each backend takes the expectations and
ignores of its manifest in `manifests/`.

The tests in `testdata/metrics/integration/gljs` are MapLibre GL JS's tests of
terrain, image sources and the built-in identity projection, with the tiles and
images they load in `gljs/assets`: a manifest's `asset_path` names a directory
whose files load in place of the cache's resources at the same `local://` paths.
The tests beside that directory (in `gljs`) load the directories it holds
(`tiles`, `geojson`, `image`) only from it, as GL JS's test server does, so a
file missing there is not found; the other tests fall back to the cache.
`mise run update:gljs-testdata <checkout>` copies them again.

`expectations/` holds mln-rs's own expected images, at the paths of the tests
they replace the images of. Every manifest reads it last, so that its image
wins. It is for tests that mln-rs draws as it means to but that the upstream
image draws otherwise, such as GL JS's terrain tests, whose heights GL JS scales
on a sphere of the earth's mean radius and mln-rs on the WGS84 radius.

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core.
