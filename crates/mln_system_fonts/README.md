# mln-rs-system-fonts

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_system_fonts`.

Finds the system's fonts for the text that a style's own fonts don't draw, which
the map shapes with them: the scripts that glyph PBFs can't shape, like
Devanagari or Myanmar, and all the text of a style without glyph PBFs. It also
draws the glyphs a style's PBFs lack, and CJK, one at a time. It looks fonts up
by a font stack's names, or by the character and the user's languages, through
the platform's font matcher where it has one.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
