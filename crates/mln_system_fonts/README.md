# mln-rs-system-fonts

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because
> mln-rs depends on it, and its API can change in any release without notice.
> Depend on [`mln-rs`](https://crates.io/crates/mln-rs) instead: it re-exports
> this crate as `mln_rs::mln_system_fonts`.

Finds the system's fonts for the text that a style's own fonts don't draw: the
scripts that glyph PBFs can't shape, like Devanagari or Myanmar, and the glyphs
of a style without glyph PBFs, those its PBFs lack, and CJK. It looks fonts up
by a font stack's names, or by the character and the user's languages, through
the platform's font matcher where it has one. MapLibre Native draws such text
through each platform's text API instead.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)).

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
