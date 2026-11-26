//! The browser build's font-face shaper: `mln_engine`'s `Shaper` in a wasm
//! module of its own. The map's Web Worker (examples/web) imports it the
//! first time a style declares font faces, a second instance beside its own,
//! and calls it through JavaScript as its map's `FontFaceEngine`
//! (examples/web/src/shaper.rs). Maps without font faces never download
//! it.

#![cfg(target_arch = "wasm32")]

use std::num::NonZeroU16;

use mln_engine::text::glyph::{FontData, GlyphID, GlyphIDType};
use mln_engine::text::{ShapedText, Shaper};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn error(message: &str);
}

/// A panic traps the instance, and the caller's binding turns the trap into
/// an error; the message goes to the console first.
#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|info| error(&info.to_string())));
}

/// Opens face `face_index` of the font in `data`, a variable font at the
/// OpenType `weight`; `undefined` unless `data` is a font with that face.
/// The face keeps `data` instead of copying it.
#[wasm_bindgen(js_name = openFace)]
#[must_use]
pub fn open_face(type_: u16, data: Vec<u8>, face_index: u32, weight: u16) -> Option<Face> {
    let data = FontData::from(data).with_face_index(face_index);
    let type_ = GlyphIDType::face(NonZeroU16::new(type_)?);
    let shaper = Shaper::at_weight(type_, data, f32::from(weight));
    shaper.valid().then_some(Face { shaper, type_ })
}

/// A font file `openFace` opened.
#[wasm_bindgen]
pub struct Face {
    shaper: Shaper,
    type_: GlyphIDType,
}

#[wasm_bindgen]
impl Face {
    /// Shapes `text`: per glyph, its index, then its x offset, y offset and
    /// advance.
    pub fn shape(&self, text: &str, rtl: bool) -> Vec<f32> {
        let text: Vec<char> = text.chars().collect();
        let mut shaped = ShapedText::default();
        self.shaper.shape(&text, rtl, &mut shaped);
        shaped
            .glyphs
            .iter()
            .zip(&shaped.adjusts)
            .flat_map(|(id, adjust)| [f32::from(id.code), adjust.x_offset, adjust.y_offset, adjust.advance])
            .collect()
    }

    /// The glyph the font maps `c` to, unshaped; `undefined` unless the
    /// face draws one.
    #[wasm_bindgen(js_name = glyphOf)]
    #[must_use]
    pub fn glyph_of(&self, c: char) -> Option<u16> {
        self.shaper.glyph_of(c).map(|glyph| glyph.code)
    }

    /// Rasterizes the glyph `code` as `[metrics, bitmap]`, the metrics a
    /// `Float64Array` of width, height, left, top, advance, double resolution
    /// (0 or 1), and the bitmap's width and height, the bitmap a
    /// `Uint8Array`.
    pub fn rasterize(&self, code: u16) -> js_sys::Array {
        let glyph = self.shaper.rasterize_glyph(GlyphID::with_type(code, self.type_));
        let m = &glyph.metrics;
        let size = glyph.bitmap.size;
        let metrics = [
            f64::from(m.width),
            f64::from(m.height),
            f64::from(m.left),
            f64::from(m.top),
            f64::from(m.advance),
            f64::from(u8::from(m.is_double_resolution)),
            f64::from(size.width),
            f64::from(size.height),
        ];
        let metrics = js_sys::Float64Array::from(&metrics[..]);
        let bitmap = js_sys::Uint8Array::from(&glyph.bitmap.data[..]);
        js_sys::Array::of2(&metrics, &bitmap)
    }
}
