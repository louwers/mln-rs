//! Text shaping and glyphs.
//!
//! Font faces shape with HarfRust and draw their glyphs from skrifa's
//! hinted outlines. `shaper.rs` says where HarfRust's shaping differs from
//! the C++ core's HarfBuzz. Arabic text drawn with glyph PBFs takes
//! presentation forms instead (`arabic.rs`).
//!
//! Glyph requests (`glyph_store`), the font-face engine
//! ([`font_face_engine`]), the device's fonts ([`system_fonts`], with the
//! `system-fonts` feature) and symbol geometry are the engine's own.

mod arabic;
#[rustfmt::skip]
mod arabic_tables;
pub mod atlas;
mod bidi;
#[rustfmt::skip]
mod bidi_mirroring;
#[cfg(target_family = "wasm")]
pub mod browser_glyph_rasterizer;
pub(crate) mod check_max_angle;
pub mod constants;
pub(crate) mod cross_tile_symbol_index;
mod face;
pub mod font_face_engine;
pub mod font_name;
pub mod font_stack;
pub(crate) mod get_anchors;
pub mod glyph;
pub mod glyph_cache;
pub mod glyph_pbf;
pub(crate) mod glyph_store;
mod line_breaks;
pub mod local_fonts;
pub(crate) mod quads;
mod rasterize;
mod shaper;
pub mod shaping;
#[cfg(test)]
mod shaping_test;
#[cfg(feature = "system-fonts")]
pub mod system_fonts;
pub mod tagged_string;
pub mod tiny_sdf;
pub mod weight;
mod woff;

pub use arabic::shape_arabic;
pub use bidi::{BiDiRun, starts_right_to_left, visual_runs};
pub use face::FontFile;
pub use glyph::{
    Bounds, Glyph, GlyphID, GlyphIDType, GlyphMap, GlyphMetrics, GlyphPosition, GlyphPositions, GlyphRange,
    PositionedGlyph, PositionedLine, Shaping,
};
pub use glyph_cache::GlyphCache;
pub use shaper::{GlyphAdjust, ShapedText, Shaper};
pub use shaping::{PositionedIcon, ShapingOptions, anchor_alignment, anchor_justification, shape_text};
pub use tagged_string::{SectionOptions, TaggedString};

#[cfg(test)]
mod tests;
