# mln-rs-style

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_style`.

Ports the style (`src/mln/style`, `include/mln/style`): `Style`, the style
parser, layers, sources, the property system, the expression language
(`src/mln/style/expression`), the feature interface that expressions read, the
spec's value types and the conversions (`mln::style::conversion`, on serde_json
instead of RapidJSON), and the utilities that the style shares with the engine
(`src/mln/util`, `include/mln/util`): errors, colors, padding, sizes,
coordinates, tilesets, image buffers, the spec's constants and more. It doesn't
need the renderer, the text engine or the tile decoders.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)), except `unitbezier.rs`, a port from WebKit
that keeps its own BSD 2-Clause License (Apple) in its header.

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
