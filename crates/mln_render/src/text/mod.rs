//! Glyphs: the store that loads and rasterizes the glyphs that layouts
//! need and shapes their text with font faces, and the engine that shapes
//! with font files. Shaping, glyph PBFs and BiDi are in the low `mln_text`
//! crate.

pub mod font_face_engine;
pub(crate) mod glyph_store;
