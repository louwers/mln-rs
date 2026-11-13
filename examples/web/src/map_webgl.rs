//! The `webgl` build's side of map.rs: the map draws with the browser's
//! WebGL2 through `mln_gl`, for browsers without WebGPU. The build without the
//! feature has none of this.

pub(super) use mln_gl::CanvasTarget as Target;
use mln_util::image::PremultipliedImage;

// The imports and the private items of map.rs.
use super::{JsValue, Rc, RendererBackend, Size, WebMap, js_error, physical, wasm_bindgen};

/// What map.rs asks of a WebGPU surface, which a canvas's WebGL2 context has
/// no use for: the map's Web Worker reads a frame from the canvas itself
/// (web/map-worker.ts), since a WebGL2 canvas can't be copied from on the
/// GPU.
pub(super) trait Readback {
    fn request_readback(&self) {}
    fn take_readback(&self) -> Option<PremultipliedImage> {
        None
    }
}

impl Readback for Target {}

#[wasm_bindgen]
impl WebMap {
    /// Creates a WebGL2 context on `canvas` and the map, as the build
    /// without the `webgl` feature creates a WebGPU device (map.rs, which
    /// describes the arguments).
    #[allow(clippy::too_many_arguments)]
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "JavaScript gets a promise from `create`, as from the WebGPU build's"
    )]
    pub async fn create(
        canvas: web_sys::OffscreenCanvas,
        width: u32,
        height: u32,
        pixel_ratio: f64,
        ports: js_sys::Array,
        on_fatal: js_sys::Function,
        load_shaper: js_sys::Function,
        local_ideograph_font_family: Option<String>,
    ) -> Result<WebMap, JsValue> {
        if ports.length() == 0 {
            return Err(js_error("the map needs at least one tile worker"));
        }
        let size = Size::new(width.max(1), height.max(1));
        let (gl_backend, target) = Target::with_backend(canvas, physical(size, pixel_ratio))
            .map_err(|e| js_error(format!("WebGL2 context: {e}")))?;
        let backend: Rc<dyn RendererBackend> = gl_backend;
        WebMap::build(backend, target, size, pixel_ratio, &ports, on_fatal, load_shaper, local_ideograph_font_family)
    }
}
