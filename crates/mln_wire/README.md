# mln-rs-wire

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_wire`.

The worker codec: a compact binary encoding for the values that cross between
the wasm instances of the browser build, such as a layer type's buckets, which
implement its `Wire` trait. It has the trait, over a byte writer and a slice
reader, the `Context` that values get registries and side tables from, and
`#[derive(Wire)]`.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
