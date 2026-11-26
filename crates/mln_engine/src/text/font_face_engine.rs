// What GlyphStore shapes and rasterizes font files with: `Shaper`
// behind a trait, so that a host can load it elsewhere.
//
// Natively, and for any build with the `font-face-shaper` feature, the
// engine is `Shaper` in-process, ready at once. The browser build
// (examples/web) turns the feature off, which leaves HarfRust, skrifa and
// the glyph rasterizer out of its module, and sets an engine that loads them
// as a second wasm module the first time a style declares font faces
// (crates/mln_web_shaper).

use std::sync::Arc;

use crate::text::ShapedText;
use crate::text::glyph::{FontData, Glyph, GlyphID, GlyphIDType};

/// A font file loaded for shaping: what `Shaper` does for `GlyphStore`.
pub trait FontFaceShaper {
    /// Appends the glyphs of `text`, one run of one direction in logical
    /// order, in visual order, and their positions.
    fn shape(&self, text: &[char], rtl: bool, shaped: &mut ShapedText);

    /// The glyph the font maps `c` to, unshaped: `None` unless it has one
    /// that `rasterize` draws.
    fn glyph_of(&self, c: char) -> Option<GlyphID>;

    /// Rasterizes the glyph: its coverage at 24 px, before the SDF transform.
    fn rasterize(&self, glyph: GlyphID) -> Glyph;
}

/// Loads font files as `FontFaceShaper`s.
pub trait FontFaceEngine: Send + Sync {
    /// Gets the engine ready to `load`, if it isn't yet, and then calls
    /// `ready` with whether it is: before returning, or later from any
    /// thread. It may be called again, before or after `ready` was called.
    fn prepare(&self, ready: Box<dyn FnOnce(Result<(), String>) + Send>);

    /// Loads a font, a variable font at the OpenType `weight`, once
    /// `prepare` reported the engine ready: `None` when `data` isn't a font
    /// the engine can read. The shaper may keep `data` (the response's
    /// bytes) instead of copying it.
    fn load(&self, type_: GlyphIDType, data: FontData, weight: u16) -> Option<Box<dyn FontFaceShaper>>;
}

/// The engine of a renderer that wasn't given one
/// (`Renderer::set_font_face_engine`): `Shaper` with the
/// `font-face-shaper` feature, else one that loads nothing.
#[cfg(feature = "font-face-shaper")]
#[must_use]
pub fn default_engine() -> Arc<dyn FontFaceEngine> {
    Arc::new(in_process::ShaperEngine)
}

/// Without the shaper, a style's font faces load nothing until a host sets
/// an engine: their text is left out, as for a font file that fails to
/// parse.
#[cfg(not(feature = "font-face-shaper"))]
#[must_use]
pub fn default_engine() -> Arc<dyn FontFaceEngine> {
    struct NoEngine;
    impl FontFaceEngine for NoEngine {
        fn prepare(&self, ready: Box<dyn FnOnce(Result<(), String>) + Send>) {
            ready(Err("this build has no font-face shaper (mln_engine's font-face-shaper feature), \
                       and the renderer was given no engine"
                .to_owned()));
        }

        fn load(&self, _type: GlyphIDType, _data: FontData, _weight: u16) -> Option<Box<dyn FontFaceShaper>> {
            None
        }
    }
    Arc::new(NoEngine)
}

#[cfg(feature = "font-face-shaper")]
mod in_process {
    use super::{FontFaceEngine, FontFaceShaper};
    use crate::text::glyph::{FontData, Glyph, GlyphID, GlyphIDType};
    use crate::text::{ShapedText, Shaper};

    /// `Shaper` in this thread.
    pub(super) struct ShaperEngine;

    impl FontFaceEngine for ShaperEngine {
        fn prepare(&self, ready: Box<dyn FnOnce(Result<(), String>) + Send>) {
            ready(Ok(()));
        }

        fn load(&self, type_: GlyphIDType, data: FontData, weight: u16) -> Option<Box<dyn FontFaceShaper>> {
            let shaper = Shaper::at_weight(type_, data, f32::from(weight));
            shaper.valid().then(|| -> Box<dyn FontFaceShaper> { Box::new(shaper) })
        }
    }

    impl FontFaceShaper for Shaper {
        fn shape(&self, text: &[char], rtl: bool, shaped: &mut ShapedText) {
            Shaper::shape(self, text, rtl, shaped);
        }

        fn glyph_of(&self, c: char) -> Option<GlyphID> {
            Shaper::glyph_of(self, c)
        }

        fn rasterize(&self, glyph: GlyphID) -> Glyph {
            self.rasterize_glyph(glyph)
        }
    }
}
