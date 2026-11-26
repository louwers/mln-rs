# mln-rs-util

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_util`.

Ports the engine's utilities of the C++ core (`src/mln/util`,
`include/mln/util`): the camera's geometry and math, threads, task queues and
pools, HTTP headers and dates, compression and SDF glyphs. The utilities that
the style shares with the engine, such as errors, colors, sizes, coordinates and
tilesets, are mln-rs-style's.

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
