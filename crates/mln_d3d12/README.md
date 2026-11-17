# mln-rs-d3d12

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_d3d12`.

The D3D12 backend: mln-rs's GPU interface on D3D12 through the `windows` crate,
and the targets that frames are drawn into. Its shaders are the HLSL that
mln-rs-shaders writes of each program, which DXC compiles at run time, so apps
that use it ship DXC's `dxcompiler.dll`, 1.8.2502 or later, which signs what it
compiles; an older one needs `dxil.dll` beside it. It is empty except on
Windows.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
