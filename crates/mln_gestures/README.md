# mln-rs-gestures

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::mln_gestures`.

Turns pointer, touch, wheel and keyboard input into camera moves and gesture
momentum, for hosts that have no gesture recognizer of their own (the GLFW app,
the browser build). MapLibre Native has no counterpart: its platform SDKs each
recognize gestures. This one is ported from
[Orbis](https://github.com/sargunv/orbis)'s `orb-camera` input.

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
