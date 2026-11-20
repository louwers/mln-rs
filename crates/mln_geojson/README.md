# mln-rs-geojson

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_geojson`.

Prepares a GeoJSON source's features for drawing: clusters its points, as
supercluster does.

Its code is under the root license ([LICENSE.md](LICENSE.md)); the algorithms it
follows are Mapbox's, under the ISC License ([LICENSE-ISC](LICENSE-ISC)).

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
