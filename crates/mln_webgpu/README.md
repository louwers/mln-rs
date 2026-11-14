# mln-rs-webgpu

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_webgpu`.

Ports the C++ WebGPU backend (`src/mln/webgpu`, its shaders and the headless
backend) onto the `wgpu` crate.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
