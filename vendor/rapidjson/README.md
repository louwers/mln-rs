# mln-rs-rapidjson

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::rapidjson`.

Ports the parts of RapidJSON that MapLibre Native's output depends on byte for byte: its number formatting and compact writer. JSON parsing uses serde_json.

Licensed under the MIT License, as RapidJSON is ([LICENSE.txt](LICENSE.txt),
RapidJSON's own license file: the parts it puts under other licenses are
not ported).

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
