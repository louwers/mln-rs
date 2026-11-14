# mln-rs-gl

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: with its `gl`
> feature it re-exports this crate as `mln_rs::mln_gl`.

Ports the C++ OpenGL backend (`src/mln/gl`, its shaders and the headless
backend) onto OpenGL ES 3.0 and desktop OpenGL 3.3 through `glow`: natively in
an EGL context or, on Windows, a WGL one, and in a browser as WebGL2 on a
canvas.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
