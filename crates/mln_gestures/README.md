# mln-rs-gestures

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_gestures`.

Recognizes pointer gestures, touch gestures, platform pinches and camera
movement from wheels, keys and other controls. It moves a `GestureCamera`, such
as `mln_runtime::Map`, and reports taps, long presses and haptic notches as
`GestureEvent`s.

Hosts translate platform events into `Input`s, choosing button and modifier
modes and converting wheel and key input into camera units. Continuous deltas
apply immediately; discrete deltas ease and accumulate across a burst.

To use it, create a `GestureRecognizer` with a camera, pass inputs to `handle`
with timestamps in milliseconds, and call `tick` when `next_tick` requests a
display frame or deadline. Drain its events to handle taps and play haptic
feedback. The recognizer owns no clock or thread.

`GestureOptions` controls which camera parts gestures move, their momentum,
snapping and haptic notches, and the zoom amounts for double and two-finger
taps. See the Rust API docs for the input units and scheduling contracts.

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
