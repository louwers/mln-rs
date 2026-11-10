# mln-rs-platform-default-image

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::mln_platform_default_image`.

Ports the default platform's image codecs (`decodeImage`, `encodePNG`) and the `util::compress` they use, and
`decoder`, through which the map decodes images and a host (the browser) can set a decoder of its own.

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
