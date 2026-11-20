# mln-rs-layers

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_layers`.

The built-in layer types, outside the engine: each type's properties, its
style-side and render-side type, its layout and bucket, its drawing, and its
programs with their shader sources in each shading language of the crate's
features. They register through `register_layer_types`. With the
`gpu-conformance` feature, `conformance` holds the frames that the GPU backends'
tests draw.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
