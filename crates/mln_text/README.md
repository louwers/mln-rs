# mln-rs-text

> **Warning: this is an internal crate of [mln-rs](https://crates.io/crates/mln-rs).**
> It is published only because mln-rs depends on it, and its API can change in
> any release without notice. Depend on [`mln-rs`](https://crates.io/crates/mln-rs)
> instead: it re-exports this crate as `mln_rs::mln_text`.

Ports text shaping and glyphs (`src/mln/text`) on HarfRust and unicode-bidi, with the FreeType and ICU behaviour the C++ core depends on.

Licensed under the BSD 2-Clause License, as the rest of mln-rs is
([LICENSE.md](../../LICENSE.md)), except the ports that keep their own licenses:
the FreeType subset (`ft`, and the WOFF decoder) under the FreeType License
([LICENSE-FreeType](LICENSE-FreeType)), hb-ft under HarfBuzz's license
([LICENSE-HarfBuzz](LICENSE-HarfBuzz)),
and ICU's BiDi properties and Arabic shaping (`icu`) under ICU's license
([LICENSE-ICU](LICENSE-ICU)), whose Unicode License V3 also covers the
Unicode data in the tables.

Part of [mln-rs](https://github.com/louwers/mln-rs),
a Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core. All its crates are released together, at the same version.
