# mln-rs-d3d12

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** Its API can change at any time
> without notice.

The D3D12 backend, so far its shaders: mln-rs-glsl's GLSL, which naga writes as
HLSL and DXC compiles at run time. Apps that use it ship DXC's
`dxcompiler.dll` and `dxil.dll`. It is empty except on Windows.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core.
