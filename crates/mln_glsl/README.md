# mln-rs-glsl

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it.

The GLSL that the Vulkan and D3D12 backends draw with, the tables of its vertex
inputs and textures, its binding layout, and naga's front end for it. It is
empty on Apple platforms and in the browser build.

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
