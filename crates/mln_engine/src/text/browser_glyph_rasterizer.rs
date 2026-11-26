//! The browser's local glyph rasterizer: draws a glyph with an
//! `OffscreenCanvas` 2D context, as MapLibre GL JS draws CJK glyphs in its
//! `localIdeographFontFamily` (with TinySDF, which `GlyphStore` applies
//! here).
//!
//! The bitmap and metrics are those of C++'s Darwin and Qt rasterizers: a
//! 24px em drawn at 2x into a 60 × 60 bitmap, a 3px border, and the glyph's
//! top measured per character. The font is each family of the font stack's
//! names, then the rasterizer's own CSS font family, which the browser falls
//! back from for a character the others lack. The weight and style come from
//! the stack's first name ([`font_name`](crate::text::font_name)).

use std::cell::RefCell;

use js_sys::Reflect;
use mln_style::bitmap::AlphaImage;
use mln_style::size::Size;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{OffscreenCanvas, OffscreenCanvasRenderingContext2d};

use crate::log_target;
use crate::text::constants::ONE_EM;
use crate::text::font_stack::FontStack;
use crate::text::glyph::{Glyph, GlyphID, GlyphMetrics};
use crate::text::local_fonts::LocalGlyphRasterizer;

// 2x bitmap with 1x logical metrics.
const TEXTURE_SCALE: u32 = 2;
const FONT_SIZE: f64 = ONE_EM as f64 * TEXTURE_SCALE as f64;
const LOGICAL_SIZE: u32 = 30;
const BITMAP_SIZE: u32 = LOGICAL_SIZE * TEXTURE_SCALE;
const RASTER_BUFFER: f64 = Glyph::BORDER_SIZE as f64 * TEXTURE_SCALE as f64;
const TOP_ADJUSTMENT: f64 = 26.5;

/// Draws glyphs with the browser's fonts.
#[derive(Debug)]
pub struct BrowserGlyphRasterizer {
    /// The CSS font family after the stack's own, such as `sans-serif`.
    font_family: String,
}

thread_local! {
    /// The thread's canvas, created at its first glyph.
    static CANVAS: RefCell<CanvasState> = const { RefCell::new(CanvasState::Unmade) };
}

enum CanvasState {
    Unmade,
    Made(Canvas),
    /// The canvas couldn't be created.
    Failed,
}

struct Canvas {
    // Kept alive with its context.
    _canvas: OffscreenCanvas,
    context: OffscreenCanvasRenderingContext2d,
    /// The context's `font`, as last set.
    font: String,
}

impl BrowserGlyphRasterizer {
    /// A rasterizer that falls back to `font_family`, a CSS font family list.
    #[must_use]
    pub fn new(font_family: String) -> Self {
        BrowserGlyphRasterizer { font_family }
    }

    /// The CSS font of the glyphs of `font_stack`.
    fn font(&self, font_stack: &FontStack) -> String {
        let first = crate::text::font_name::parse(font_stack.first().map_or("", String::as_str));
        let style = if first.italic { "italic" } else { "normal" };
        let mut families: Vec<String> = Vec::new();
        for name in font_stack {
            let family = format!("\"{}\"", crate::text::font_name::parse(name).family.replace(['"', '\\'], ""));
            if !families.contains(&family) {
                families.push(family);
            }
        }
        families.push(self.font_family.clone());
        format!("{style} {} {FONT_SIZE}px {}", first.weight, families.join(", "))
    }
}

impl LocalGlyphRasterizer for BrowserGlyphRasterizer {
    fn rasterize_glyph(&self, font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph> {
        let text = char::from_u32(u32::from(glyph_id.code))?;
        let font = self.font(font_stack);
        CANVAS.with_borrow_mut(|state| {
            if matches!(state, CanvasState::Unmade) {
                *state = match Canvas::new() {
                    Ok(canvas) => CanvasState::Made(canvas),
                    Err(error) => {
                        log::warn!(target: log_target::GLYPH, "Unable to create a canvas for local glyphs: {error:?}");
                        CanvasState::Failed
                    }
                };
            }
            let CanvasState::Made(canvas) = state else { return None };
            match canvas.draw(&font, &text.to_string()) {
                Ok((bitmap, metrics)) => Some(Glyph { id: glyph_id, bitmap, metrics }),
                Err(error) => {
                    log::warn!(target: log_target::GLYPH, "Unable to draw glyph {}: {error:?}", glyph_id.code);
                    None
                }
            }
        })
    }
}

impl Canvas {
    fn new() -> Result<Self, JsValue> {
        let canvas = OffscreenCanvas::new(BITMAP_SIZE, BITMAP_SIZE)?;
        let options = js_sys::Object::new();
        Reflect::set(&options, &"willReadFrequently".into(), &true.into())?;
        let context: OffscreenCanvasRenderingContext2d = canvas
            .get_context_with_context_options("2d", &options)?
            .ok_or_else(|| JsValue::from_str("no 2d context"))?
            .dyn_into()?;
        context.set_text_baseline("alphabetic");
        context.set_text_align("left");
        context.set_fill_style_str("black");
        Ok(Canvas { _canvas: canvas, context, font: String::new() })
    }

    /// Draws `text` in `font`, its baseline `RASTER_BUFFER` below the top of
    /// its ink, and returns the bitmap's alpha and the glyph's metrics.
    fn draw(&mut self, font: &str, text: &str) -> Result<(AlphaImage, GlyphMetrics), JsValue> {
        let context = &self.context;
        if self.font != font {
            context.set_font(font);
            self.font = font.to_owned();
        }
        let measured = context.measure_text(text)?;
        let glyph_top = measured.actual_bounding_box_ascent().ceil().max(0.0);
        let size = f64::from(BITMAP_SIZE);
        context.clear_rect(0.0, 0.0, size, size);
        context.fill_text(text, 0.0, RASTER_BUFFER + glyph_top)?;
        let rgba = context.get_image_data(0.0, 0.0, size, size)?.data().0;
        let alpha = rgba.as_chunks::<4>().0.iter().map(|texel| texel[3]).collect();

        let scale = f64::from(TEXTURE_SCALE);
        let metrics = GlyphMetrics {
            width: LOGICAL_SIZE,
            height: LOGICAL_SIZE,
            left: f32::from(Glyph::BORDER_SIZE) + 0.5,
            top: (glyph_top / scale - TOP_ADJUSTMENT) as f32,
            advance: (measured.width() / scale).round() as u32,
            is_double_resolution: true,
        };
        Ok((AlphaImage::from_vec(Size::new(BITMAP_SIZE, BITMAP_SIZE), alpha), metrics))
    }
}
