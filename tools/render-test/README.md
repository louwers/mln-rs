# mln-rs-render-test

Ports MapLibre Native's render test runner (`render-test/`), which runs the
shared render and query tests.

`cargo test -p mln-rs-render-test --test integration render_tests::` runs them
twice, in one worker process per CPU (`RENDER_TEST_JOBS=<n>` sets the number; 1
runs them in the test's own process): as they are, and with the tile workers
behind the worker codec of the browser build (`--tileWorkerCodec=2`). It writes
the actual and diff images, and HTML reports that show the failed and errored
tests, to `target/tmp/render_tests/` and
`target/tmp/render_tests_through_the_codec/`.
`cargo run -p mln-rs-render-test -- --help` lists the runner's options, such as
`--filter`, `--jobs` and `--tileWorkerCodec`.

The tests render on WebGPU, or on the backend that `MLN_RENDER_BACKEND` names:
`vulkan`, `d3d12` (Windows; DXC's `dxcompiler.dll` on the DLL search path),
`gl`, `gl-desktop`, or `metal` (macOS). Each backend takes the expectations and
ignores of its manifest in `manifests/`. Every manifest lists `expectations/`
last, so its images win over the copied ones: mln-rs's own expected images, for
tests whose copied image mln-rs differs from on purpose. `ignores/` holds
mln-rs's own ignore files, and `ignores/rust-port-unignores.json`, which every
manifest names in `unignore_paths`, the tests that run although a copied ignore
file names them.

The tests in `testdata/metrics/integration/gljs` are MapLibre GL JS's tests of
image sources, with the images they load in `gljs/assets`: a manifest's
`asset_path` names a directory whose files load in place of the cache's
resources at the same `local://` paths. The tests beside that directory (in
`gljs`) load the directories it holds (`image`) only from it, as GL JS's test
server does, so a file missing there is not found; the other tests fall back to
the cache. `mise run update:gljs-testdata <checkout>` copies them again, and
`mise run update:testdata` leaves them alone.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core.
