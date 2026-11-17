# mln-rs-frame-pacer

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_frame_pacer`.

Paces frames to a display: a `FramePacer` calls back at the display's refreshes,
at most at a frame rate limit, so that a host draws each frame when the display
is ready for it. It follows a `CADisplayLink` on Apple platforms, or a source of
the host's own.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
