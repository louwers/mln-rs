//! The `webgl` build's side of map.rs: the map draws with the browser's
//! WebGL2 through `mln_gl`, for browsers without WebGPU. The build without the
//! feature has none of this.

pub use mln_gl::CanvasTarget as Target;
use mln_util::image::PremultipliedImage;

// The imports and the private items of map.rs.
use super::{
    Arc, Cell, Frontend, GestureRecognizer, JsValue, Map, MapOptions, Rc, Renderer, RendererBackend, RunLoop,
    SerializingTransport, ShaperModuleEngine, Size, WAKE, WebMap, WebWorkers, cooperative_scheduler, font_face_engine,
    js_error, physical, shaper, wake_host, wasm_bindgen,
};

/// What map.rs asks of a WebGPU surface, which a canvas's WebGL2 context has
/// no use for: the map's Web Worker reads a frame from the canvas itself
/// (web/map-worker.ts), since a WebGL2 canvas can't be copied from on the
/// GPU.
pub trait Readback {
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
        workers: u32,
        post: js_sys::Function,
        wake: js_sys::Function,
        load_shaper: js_sys::Function,
        local_ideograph_font_family: Option<String>,
    ) -> Result<WebMap, JsValue> {
        if workers == 0 {
            return Err(js_error("the map needs at least one tile worker"));
        }
        let scheduler = cooperative_scheduler();
        shaper::set_loader(load_shaper);
        font_face_engine::set_font_face_engine(Arc::new(ShaperModuleEngine));

        let size = Size::new(width.max(1), height.max(1));
        let (gl_backend, target) = Target::with_backend(canvas, physical(size, pixel_ratio))
            .map_err(|e| js_error(format!("WebGL2 context: {e}")))?;
        let backend: Rc<dyn RendererBackend> = gl_backend;

        let run_loop = RunLoop::get();
        WAKE.with(|w| *w.borrow_mut() = Some(wake));
        run_loop.set_waker(Some(Arc::new(wake_host)));
        scheduler.set_waker(Some(Arc::new(wake_host)));
        let mut renderer = Renderer::new(backend.clone(), pixel_ratio as f32, local_ideograph_font_family);
        let count = workers as usize;
        let transport = SerializingTransport::new(|_| Box::new(WebWorkers { count, post }));
        renderer.set_tile_worker_transport(transport.clone());
        let mut map =
            Map::new(MapOptions::new().with_size(size).with_pixel_ratio(pixel_ratio as f32)).map_err(js_error)?;
        map.set_waker(Some(Arc::new(wake_host)));
        renderer.set_event_queue(Some(map.renderer_events()));
        let frontend =
            Frontend { renderer, target, backend, updates: map.update_reader(), drawn_update: 0, redraw: false };
        Ok(WebMap {
            frontend,
            map,
            loaded: false,
            run_loop,
            scheduler,
            transport,
            pixel_ratio,
            received: Cell::new(0),
            gestures: GestureRecognizer::default(),
        })
    }
}
