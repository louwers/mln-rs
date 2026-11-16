# mln-rs-video

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_video`.

Plays the videos of a style's `video` sources. The map never decodes video: a
player of the platform's (GStreamer on Linux, `MediaCodec` on Android,
AVFoundation on Apple's platforms, Media Foundation on Windows, the page's
`<video>` in the browser) decodes each video, reading its bytes through the
map's storage, and hands the frames to the map, in memory or on the GPU. Apps
and tests can bring their own player through the same traits.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)).

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
