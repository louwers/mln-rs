//! Glyph requests and symbol geometry. Shaping, glyph PBFs and BiDi are in
//! the `mln_text` crate.

pub(crate) mod check_max_angle;
pub(crate) mod cross_tile_symbol_index;
pub mod font_face_engine;
pub(crate) mod get_anchors;
pub(crate) mod glyph_store;
pub(crate) mod quads;
