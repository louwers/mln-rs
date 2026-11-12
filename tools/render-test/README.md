# mln-rs-render-test

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only for mln-rs's differential tests against the C++ core
> ([mln-rs-difftests](https://github.com/louwers/mln-rs-difftests)), and its
> API can change in any release without notice.

Ports MapLibre Native's render test runner (`render-test/`), which runs the shared render and query tests.

`cargo test -p mln-rs-render-test --test render_tests` runs them twice, in
one worker process per CPU (`RENDER_TEST_JOBS=<n>` sets the number; 1 runs
them in the test's own process): as they are, and with the tile workers
behind the worker codec of the browser build (`--tileWorkerCodec=2`). It
writes the actual and diff images, and HTML reports that show the failed
and errored tests, to `target/tmp/render_tests/` and
`target/tmp/render_tests_through_the_codec/`.
`cargo run -p mln-rs-render-test -- --help` lists the runner's options,
such as `--filter`, `--jobs` and `--tileWorkerCodec`.

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
