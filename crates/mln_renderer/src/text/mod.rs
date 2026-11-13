//! Glyph requests, symbol geometry, collision and placement. These use the
//! renderer's buckets, layers and storage, so they live in `mln_renderer`;
//! shaping, glyph PBFs and BiDi are in the low `mln_text` crate.

pub(crate) mod check_max_angle;
pub(crate) mod collision_feature;
pub(crate) mod collision_index;
pub(crate) mod cross_tile_symbol_index;
pub mod font_face_engine;
pub(crate) mod get_anchors;
pub(crate) mod glyph_manager;
pub mod placement;
pub(crate) mod quads;
