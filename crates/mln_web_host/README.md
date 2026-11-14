# mln-rs-web-host

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_web_host`.

What a wasm instance needs to run mln-rs in a browser: wasm32-unknown-unknown
without atomics or shared memory, so one instance per Web Worker, each with its
own memory and one thread. MapLibre Native has no counterpart; its browser
builds use Emscripten's threads.

- `pump`: runs the instance's `RunLoop`s and its cooperative scheduler on the
  browser's event loop, in slices of a few milliseconds that never block it.
- `tile_workers` and `tile_worker`: the two sides of tile workers in other Web
  Workers, which talk to the map's instance only through bytes over
  `MessagePort`s (mln_renderer's `SerializingTransport` and `TileWorkerHost`).
- `shaper`: the font-face shaper as a second module (`crates/mln_web_shaper`),
  which the map's instance loads the first time a style declares font faces.

examples/web runs a map with them; its README describes the model. The crate is
empty on other targets.
