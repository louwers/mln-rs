# mln-rs-wagyu

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::wagyu`.

Ports wagyu 0.5.0 (`mapbox::geometry::wagyu`), the polygon clipper the C++ core fixes up tile polygons with.

Licensed under the Boost Software License 1.0, as wagyu is
([LICENSE](LICENSE)), except `almost_equal.rs`, which keeps Google Test's
BSD 3-Clause License (in its header).

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
