# mln-rs-util

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::mln_util`.

Ports the utilities of the C++ core (`src/mln/util`, `include/mln/util`).

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)), except the ports that keep their own licenses:
`mat2.rs`, `mat3.rs` and `mat4.rs` from gl-matrix (zlib License) and
`unitbezier.rs` from WebKit (BSD 2-Clause License, Apple), each in its
header, and `parsedate.rs` from curl ([LICENSE-curl](LICENSE-curl)).

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
