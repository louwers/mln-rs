# mln-rs-nunicode

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::nunicode`.

Ports the C++ core's subset of nunicode: DUCET collation and unaccent, which the default platform's collator uses.

Licensed under the MIT License, as nunicode is ([LICENSE](LICENSE)). The
tables hold Unicode's data, under the Unicode License V3
([LICENSE-Unicode](LICENSE-Unicode)), and the functions from MapLibre
Native are under its BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)).

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
