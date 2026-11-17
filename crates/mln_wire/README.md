# mln-rs-wire

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it.

A compact binary encoding for the values that cross between the wasm instances
of the browser build: a `Wire` trait over a byte writer and a slice reader, the
`Context` that values get registries and side tables from, and
`#[derive(Wire)]`.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
