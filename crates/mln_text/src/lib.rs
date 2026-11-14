//! Text shaping and glyphs, with the parts of ICU that the BiDi code uses
//! (`icu`).
//!
//! Font faces shape with HarfRust and draw their glyphs from skrifa's
//! hinted outlines. `harfbuzz_impl.rs` says where HarfRust's shaping differs
//! from the C++ core's HarfBuzz.

mod bidi;
#[rustfmt::skip]
mod bidi_mirroring;
#[cfg(target_family = "wasm")]
mod browser_glyph_rasterizer;
mod face;
pub mod glyph;
pub mod glyph_cache;
pub mod glyph_pbf;
mod harfbuzz;
mod harfbuzz_impl;
mod icu;
pub mod local_glyph_rasterizer;
mod rasterize;
pub mod shaping;
#[cfg(test)]
mod shaping_test;
pub mod tagged_string;
pub mod weight;
mod woff;

pub use bidi::{BiDi, apply_arabic_shaping, starts_right_to_left};
pub use face::FreeTypeLibrary;
pub use glyph::{
    Glyph, GlyphID, GlyphIDType, GlyphMap, GlyphMetrics, GlyphPosition, GlyphPositions, GlyphRange, PositionedGlyph,
    PositionedLine, Shaping, WritingModeType,
};
pub use glyph_cache::GlyphCache;
pub use harfbuzz::{HBShapeAdjust, HBShaper};
pub use shaping::{AnchorAlignment, PositionedIcon, get_anchor_justification, get_shaping};
pub use tagged_string::{SectionOptions, TaggedString};

#[cfg(test)]
mod tests;
