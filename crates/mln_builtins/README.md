# mln-rs-builtins

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_builtins`.

The style specification's source types, through the engine's source type
contract: vector, raster and raster-dem tilesets, GeoJSON and image sources, and
how they load. `register_builtins` fills a registry with them and the built-in
layer types.

Its code is under the root license ([LICENSE.md](LICENSE.md)); the symbol
layout's label points follow Mapbox's polylabel, under the ISC License
([LICENSE-ISC](LICENSE-ISC)).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
