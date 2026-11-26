//! A map's font faces (a style's "font-faces"), shaped and rasterized by
//! the shaper module (`crates/mln_web_shaper`): the map's `FontFaceEngine`.
//! A module built without `mln_engine`'s in-process shaper leaves HarfRust,
//! skrifa and the glyph rasterizer to that one, which the map's instance
//! loads the first time a style declares font faces, as a second wasm
//! instance beside it. Once it is loaded, calls go to it synchronously,
//! through JavaScript.

use std::cell::RefCell;

use mln_engine::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use mln_style::bitmap::AlphaImage;
use mln_style::size::Size;
use mln_text::glyph::{FontData, Glyph, GlyphID, GlyphIDType, GlyphMetrics};
use mln_text::{GlyphAdjust, ShapedText};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::log_target;

// A panic in the shaper traps its instance, and the trap is a JavaScript
// exception here. Each call catches it, as an error: thrown through this
// module, it would unwind past the RefCell borrows of the callers.
#[wasm_bindgen]
extern "C" {
    /// The shaper module, initialized.
    type ShaperModule;

    #[wasm_bindgen(method, catch, js_name = openFace)]
    fn open_face(
        this: &ShaperModule,
        type_: u16,
        data: &[u8],
        face_index: u32,
        weight: u16,
    ) -> Result<Option<Face>, JsValue>;

    /// A font file it opened.
    type Face;

    #[wasm_bindgen(method, catch)]
    fn shape(this: &Face, text: &str, rtl: bool) -> Result<Vec<f32>, JsValue>;

    #[wasm_bindgen(method, catch, js_name = glyphOf)]
    fn glyph_of(this: &Face, c: char) -> Result<Option<u16>, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn rasterize(this: &Face, code: u16) -> Result<js_sys::Array, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn free(this: &Face) -> Result<(), JsValue>;
}

type Ready = Box<dyn FnOnce(Result<(), String>) + Send>;

/// Where the shaper module is in this instance.
enum Module {
    /// Not loaded yet; calling `loader` returns a promise of the module.
    NotLoaded {
        loader: js_sys::Function,
    },
    /// Loading; these `prepare`s wait for it.
    Loading {
        waiting: Vec<Ready>,
    },
    Loaded(ShaperModule),
}

thread_local! {
    static MODULE: RefCell<Option<Module>> = const { RefCell::new(None) };
}

/// Sets how this instance loads the shaper module: `loader()` returns a
/// promise of the module, initialized. It has no effect once the module is
/// loading or loaded (for an earlier map).
pub fn set_loader(loader: js_sys::Function) {
    MODULE.with(|module| {
        let mut module = module.borrow_mut();
        if matches!(*module, None | Some(Module::NotLoaded { .. })) {
            *module = Some(Module::NotLoaded { loader });
        }
    });
}

/// Sets how this instance loads the shaper module, unless a loader is set
/// already: for the package that ships the module beside its own.
pub fn set_default_loader(loader: js_sys::Function) {
    MODULE.with(|module| {
        let mut module = module.borrow_mut();
        if module.is_none() {
            *module = Some(Module::NotLoaded { loader });
        }
    });
}

/// [`set_loader`], for the JavaScript that hosts the instance.
#[wasm_bindgen(js_name = mlnWebSetShaperLoader)]
pub fn set_loader_js(loader: js_sys::Function) {
    set_loader(loader);
}

fn message(error: &JsValue) -> String {
    match error.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

/// The `FontFaceEngine` on the shaper module. Its state is the instance's
/// (`MODULE`): an instance has one thread.
pub struct ShaperModuleEngine;

impl FontFaceEngine for ShaperModuleEngine {
    fn prepare(&self, ready: Ready) {
        enum Step {
            NoLoader,
            Ready,
            Wait,
            Load(js_sys::Function),
        }
        let step = MODULE.with(|module| {
            let mut module = module.borrow_mut();
            match &*module {
                None => Step::NoLoader,
                Some(Module::Loaded(_)) => Step::Ready,
                Some(Module::Loading { .. }) => Step::Wait,
                Some(Module::NotLoaded { loader }) => {
                    let loader = loader.clone();
                    *module = Some(Module::Loading { waiting: Vec::new() });
                    Step::Load(loader)
                }
            }
        });
        match step {
            Step::NoLoader => return ready(Err("the map has no shaper module loader".to_owned())),
            Step::Ready => return ready(Ok(())),
            Step::Wait | Step::Load(_) => MODULE.with(|module| {
                if let Some(Module::Loading { waiting }) = &mut *module.borrow_mut() {
                    waiting.push(ready);
                }
            }),
        }
        let Step::Load(loader) = step else { return };
        let promise = loader.call0(&JsValue::NULL).and_then(JsCast::dyn_into::<js_sys::Promise>);
        wasm_bindgen_futures::spawn_local(async move {
            let loaded = match promise {
                Ok(promise) => JsFuture::from(promise).await,
                Err(error) => Err(error),
            };
            let (waiting, result) = MODULE.with(|module| {
                let mut module = module.borrow_mut();
                let waiting = match module.take() {
                    Some(Module::Loading { waiting }) => waiting,
                    _ => Vec::new(),
                };
                match loaded {
                    Ok(loaded) => {
                        *module = Some(Module::Loaded(loaded.unchecked_into()));
                        (waiting, Ok(()))
                    }
                    Err(error) => {
                        // A later prepare (another map) tries again.
                        *module = Some(Module::NotLoaded { loader });
                        (waiting, Err(format!("the shaper module failed to load: {}", message(&error))))
                    }
                }
            });
            for ready in waiting {
                ready(result.clone());
            }
        });
    }

    fn load(&self, type_: GlyphIDType, data: FontData, weight: u16) -> Option<Box<dyn FontFaceShaper>> {
        let opened = MODULE.with(|module| match &*module.borrow() {
            Some(Module::Loaded(module)) => module.open_face(type_.id(), data.as_bytes(), data.face_index(), weight),
            _ => Err(JsValue::from_str("the shaper module isn't loaded")),
        });
        match opened {
            Ok(face) => face.map(|face| -> Box<dyn FontFaceShaper> { Box::new(ShaperFace { face, type_ }) }),
            Err(error) => {
                log::error!(target: log_target::GLYPH, "opening a font face failed: {}", message(&error));
                None
            }
        }
    }
}

/// A font file open in the shaper module.
struct ShaperFace {
    face: Face,
    type_: GlyphIDType,
}

impl FontFaceShaper for ShaperFace {
    fn shape(&self, text: &[char], rtl: bool, shaped: &mut ShapedText) {
        match self.face.shape(&text.iter().collect::<String>(), rtl) {
            Ok(glyphs) => {
                for glyph in glyphs.as_chunks::<4>().0 {
                    shaped.glyphs.push(GlyphID::with_type(glyph[0] as u16, self.type_));
                    shaped.adjusts.push(GlyphAdjust::new(glyph[1], glyph[2], glyph[3]));
                }
            }
            Err(error) => {
                log::error!(target: log_target::GLYPH, "shaping with a font face failed: {}", message(&error));
            }
        }
    }

    fn glyph_of(&self, c: char) -> Option<GlyphID> {
        match self.face.glyph_of(c) {
            Ok(code) => Some(GlyphID::with_type(code?, self.type_)),
            Err(error) => {
                log::error!(target: log_target::GLYPH, "finding a font face's glyph failed: {}", message(&error));
                None
            }
        }
    }

    fn rasterize(&self, glyph: GlyphID) -> Glyph {
        let empty = Glyph { id: glyph, ..Glyph::default() };
        let parts = match self.face.rasterize(glyph.code) {
            Ok(parts) => parts,
            Err(error) => {
                log::error!(target: log_target::GLYPH, "rasterizing a font face's glyph failed: {}", message(&error));
                return empty;
            }
        };
        let metrics = js_sys::Float64Array::new(&parts.get(0)).to_vec();
        let bitmap = js_sys::Uint8Array::new(&parts.get(1)).to_vec();
        let [width, height, left, top, advance, double_resolution, bitmap_width, bitmap_height] = metrics[..] else {
            return empty;
        };
        let Ok(bitmap) = AlphaImage::from_slice(Size::new(bitmap_width as u32, bitmap_height as u32), &bitmap) else {
            return empty;
        };
        Glyph {
            id: glyph,
            bitmap,
            metrics: GlyphMetrics {
                width: width as u32,
                height: height as u32,
                left: left as f32,
                top: top as f32,
                advance: advance as u32,
                is_double_resolution: double_resolution != 0.0,
            },
        }
    }
}

impl Drop for ShaperFace {
    fn drop(&mut self) {
        let _ = self.face.free();
    }
}
