# mln-rs-protozero

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::protozero`.

Ports the reader half of protozero 1.7.1, the protocol buffer decoder behind vector tiles and glyph PBFs.

Licensed under the BSD 2-Clause License, as protozero is
([LICENSE.md](LICENSE.md)), except the varint decoder, which comes from
Facebook's folly and is under the Apache License 2.0
([LICENSE.from_folly](LICENSE.from_folly)).

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
