//! Ports the high-level files of `src/mln/text` (glyph requests, symbol
//! geometry, collision and placement). They include the renderer's
//! buckets, layers and storage, so they live in `mln_renderer`; shaping,
//! glyph PBFs and BiDi are in the low `mln_text` crate.

pub mod check_max_angle;
pub mod collision_feature;
pub mod collision_index;
pub mod cross_tile_symbol_index;
pub mod font_face_engine;
pub mod get_anchors;
pub mod glyph_manager;
pub mod placement;
pub mod quads;
