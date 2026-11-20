//! Text shaping and glyphs.
//!
//! Font faces shape with HarfRust and draw their glyphs from skrifa's
//! hinted outlines. `shaper.rs` says where HarfRust's shaping differs from
//! the C++ core's HarfBuzz. Arabic text drawn with glyph PBFs takes
//! presentation forms instead (`arabic.rs`).

mod arabic;
#[rustfmt::skip]
mod arabic_tables;
mod bidi;
#[rustfmt::skip]
mod bidi_mirroring;
#[cfg(target_family = "wasm")]
pub mod browser_glyph_rasterizer;
mod face;
pub mod font_name;
pub mod glyph;
pub mod glyph_cache;
pub mod glyph_pbf;
mod line_breaks;
pub mod local_glyph_rasterizer;
mod rasterize;
mod shaper;
pub mod shaping;
#[cfg(test)]
mod shaping_test;
pub mod tagged_string;
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
