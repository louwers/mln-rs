# mln-rs-runtime

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate's types at its root.

Thread-safe handles over mln-rs's maps, which mirror
[maplibre-native-ffi](https://github.com/maplibre/maplibre-native-ffi)'s C API
one to one. A `Runtime` owns a thread, and the file sources, cache and offline
regions that its maps share. A `Map` handle sends commands to the map on that
thread: they run in the order they were sent, and each reports through a
`Completion`, which is a future, can be waited for, or calls back. A map
publishes a snapshot of its state after each command, which any thread reads
without waiting, and the events of a runtime and its maps reach one handler in
batches. A `RenderSession` that a map attaches renders the map's frames
offscreen or onto a window's surface: the host demands frames and drains their
results, reads offscreen pixels back, and queries the features drawn. A session
renders on a thread of its own, or on the host's graphics thread, which runs the
session's work through its `DriverWork`.

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
