# mln-rs-text

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_text`.

Ports text shaping and glyphs (`src/mln/text`) on HarfRust, skrifa and
unicode-bidi.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)), except the Unicode data in the Arabic shaping
and BiDi mirroring tables (`arabic_tables.rs`, `bidi_mirroring.rs`), under the
[Unicode License V3](https://www.unicode.org/license.txt).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
