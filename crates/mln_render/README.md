# mln-rs-render

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_render`.

Ports the renderer (`src/mln/renderer`, `include/mln/renderer`), which draws the
layer and source types of the registry it is made with. Its `layer`, `draw` and
`source` modules state what a layer type and a source type implement, and its
`globe` module has the geometry that drawing on the globe needs.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
