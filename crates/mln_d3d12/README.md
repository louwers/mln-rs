# mln-rs-d3d12

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it yet.

The D3D12 backend: mln-rs's GPU interface on D3D12 through the `windows` crate,
its shaders and offscreen frames. Its shaders are mln-rs-glsl's GLSL, which naga
writes as HLSL and DXC compiles at run time, so apps that use it ship DXC's
`dxcompiler.dll` and `dxil.dll`. It is empty except on Windows.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
