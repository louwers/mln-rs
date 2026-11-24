# mln-rs-android

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it.

What the mln-rs crates' Android code shares: the Java VM that their services
call, such as HTTP through `HttpURLConnection` and the locale-sensitive
expressions through `android.icu`.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
