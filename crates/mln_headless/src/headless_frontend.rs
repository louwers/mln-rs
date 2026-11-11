// Ports platform/default/include/mln/gfx/headless_frontend.hpp and
// platform/default/src/mln/gfx/headless_frontend.cpp.
//
// C++ renders with the one backend it was built with. The port renders with
// WebGPU, or with another backend the environment variable
// MLN_RENDER_BACKEND names: `metal` on Apple platforms, and `gl` (OpenGL
// ES 3.0 through EGL) in a build with the `gl` feature.

use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_actor::{AsyncTask, RunLoop};
use mln_gfx::headless_backend::HeadlessBackend as GfxHeadlessBackend;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::{BackendScope, ContextMode, RendererBackend, ScopeType, SwapBehaviour};
use mln_map::{CameraOptions, Map, TransformState};
use mln_renderer::{Renderer, UpdateParameters, UpdateReader};
use mln_text::GlyphCache;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
#[cfg(any(target_vendor = "apple", all(feature = "gl", not(target_arch = "wasm32"))))]
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

/// `HeadlessFrontend::RenderResult`.
pub struct RenderResult {
    pub image: PremultipliedImage,
    pub stats: RenderingStats,
}

struct Shared {
    size: Cell<Size>,
    pixel_ratio: f32,
    frame_time: Cell<f64>,
    backend: Rc<dyn GfxHeadlessBackend>,
    /// `backend` as the renderer's backend.
    renderer_backend: Rc<dyn RendererBackend>,
    async_invalidate: RefCell<Option<AsyncTask>>,
    invalidate_on_update: bool,
    glyph_cache: Arc<GlyphCache>,
    /// Made when first used, and dropped when the attached map goes.
    renderer: RefCell<Option<Renderer>>,
    /// The updates of the map that the frontend renders.
    updates: RefCell<Option<UpdateReader>>,
}

/// Renders a map into an offscreen texture on the calling thread.
///
/// The frontend draws the latest update of the map it is attached to
/// ([`attach`](Self::attach), which [`render`](Self::render) does too). With
/// `invalidate_on_update`, it draws each update the map publishes on the
/// thread's `RunLoop`. When the map goes, the frontend drops its renderer
/// and what it held on the `RunLoop`, and makes another when next used.
/// Clones share the renderer.
#[derive(Clone)]
pub struct HeadlessFrontend {
    shared: Rc<Shared>,
}

/// The headless backend MLN_RENDER_BACKEND asks for (`gfx::Backend::Create`),
/// as the gfx headless backend and as the renderer's backend.
fn create_backend(
    size: Size,
    swap_behavior: SwapBehaviour,
    context_mode: ContextMode,
) -> (Rc<dyn GfxHeadlessBackend>, Rc<dyn RendererBackend>) {
    #[cfg(target_vendor = "apple")]
    if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("metal")) {
        match mln_mtl::HeadlessBackend::new(size, swap_behavior, context_mode) {
            Ok(backend) => {
                let backend = Rc::new(backend);
                return (backend.clone(), backend);
            }
            Err(e) => Log::error(Event::Render, format!("{e}; rendering with WebGPU instead")),
        }
    }
    #[cfg(all(feature = "gl", not(target_arch = "wasm32")))]
    if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("gl")) {
        match mln_gl::HeadlessBackend::new(size, swap_behavior, context_mode) {
            Ok(backend) => {
                let backend = Rc::new(backend);
                return (backend.clone(), backend);
            }
            Err(e) => Log::error(Event::Render, format!("{e}; rendering with WebGPU instead")),
        }
    }
    let backend = Rc::new(mln_webgpu::HeadlessBackend::new(size, swap_behavior, context_mode));
    (backend.clone(), backend)
}

/// `{static_cast<uint32_t>(size.width * pixelRatio), static_cast<uint32_t>(size.height * pixelRatio)}`.
fn physical_size(size: Size, pixel_ratio: f32) -> Size {
    Size::new((size.width as f32 * pixel_ratio) as u32, (size.height as f32 * pixel_ratio) as u32)
}

impl HeadlessFrontend {
    /// `HeadlessFrontend(Size, float pixelRatio)` with the default swap
    /// behaviour (NoFlush), context mode (Unique), no local font family and
    /// `invalidateOnUpdate = true`.
    pub fn new(size: Size, pixel_ratio: f32) -> Self {
        Self::with_options(size, pixel_ratio, SwapBehaviour::NoFlush, ContextMode::Unique, true)
    }

    /// `HeadlessFrontend(Size, float pixelRatio, SwapBehaviour, ContextMode,
    /// localFontFamily, invalidateOnUpdate)`: its renderer has a glyph cache
    /// of its own.
    // TODO(port): localFontFamily.
    pub fn with_options(
        size: Size,
        pixel_ratio: f32,
        swap_behavior: SwapBehaviour,
        context_mode: ContextMode,
        invalidate_on_update: bool,
    ) -> Self {
        Self::with_glyph_cache(size, pixel_ratio, swap_behavior, context_mode, invalidate_on_update, Arc::default())
    }

    /// `HeadlessFrontend(Size, float pixelRatio, SwapBehaviour, ContextMode,
    /// localFontFamily, invalidateOnUpdate, std::shared_ptr<GlyphCache>)`:
    /// frontends given the same `GlyphCache` share the glyphs their
    /// renderers parse from glyph PBFs, also across threads.
    pub fn with_glyph_cache(
        size: Size,
        pixel_ratio: f32,
        swap_behavior: SwapBehaviour,
        context_mode: ContextMode,
        invalidate_on_update: bool,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        let (backend, renderer_backend) = create_backend(physical_size(size, pixel_ratio), swap_behavior, context_mode);
        Self::with_backends(backend, renderer_backend, size, pixel_ratio, invalidate_on_update, glyph_cache)
    }

    /// Port-only: a frontend that renders through `backend`, such as one
    /// on a device other backends share (`HeadlessBackend::with_device`),
    /// which it sizes for `size` at `pixel_ratio`.
    pub fn with_backend(
        backend: mln_webgpu::HeadlessBackend,
        size: Size,
        pixel_ratio: f32,
        invalidate_on_update: bool,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        backend.set_size(physical_size(size, pixel_ratio));
        let backend = Rc::new(backend);
        Self::with_backends(backend.clone(), backend, size, pixel_ratio, invalidate_on_update, glyph_cache)
    }

    /// A frontend on `backend`, which is also the renderer's backend.
    fn with_backends(
        backend: Rc<dyn GfxHeadlessBackend>,
        renderer_backend: Rc<dyn RendererBackend>,
        size: Size,
        pixel_ratio: f32,
        invalidate_on_update: bool,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        let shared = Rc::new(Shared {
            size: Cell::new(size),
            pixel_ratio,
            frame_time: Cell::new(0.0),
            backend,
            renderer_backend,
            async_invalidate: RefCell::new(None),
            invalidate_on_update,
            glyph_cache,
            renderer: RefCell::new(None),
            updates: RefCell::new(None),
        });
        let weak: Weak<Shared> = Rc::downgrade(&shared);
        *shared.async_invalidate.borrow_mut() = Some(AsyncTask::new(move || {
            if let Some(shared) = weak.upgrade() {
                Shared::on_wake(&shared);
            }
        }));
        HeadlessFrontend { shared }
    }

    /// `HeadlessFrontend(float pixelRatio, ...)`: a 256x256 frontend.
    pub fn with_pixel_ratio(pixel_ratio: f32) -> Self {
        Self::new(Size::new(256, 256), pixel_ratio)
    }

    pub fn get_frame_time(&self) -> f64 {
        self.shared.frame_time.get()
    }

    pub fn get_size(&self) -> Size {
        self.shared.size.get()
    }

    pub fn set_size(&mut self, size: Size) {
        if self.shared.size.get() != size {
            self.shared.size.set(size);
            self.shared.backend.set_size(physical_size(size, self.shared.pixel_ratio));
        }
    }

    /// Renders `map` from now on: the frontend reads the map's updates, and
    /// its renderer reports to the map. Attaching to the map it renders
    /// already does nothing.
    pub fn attach(&self, map: &Map) {
        let reader = map.update_reader();
        if self.shared.updates.borrow().as_ref().is_some_and(|updates| updates.ptr_eq(&reader)) {
            return;
        }
        self.shared.renderer().set_event_queue(Some(map.renderer_events()));
        if let Some(task) = self.shared.async_invalidate.borrow().as_ref() {
            let sender = task.sender();
            reader.set_waker(Some(Arc::new(move || sender.send())));
            if self.shared.invalidate_on_update && reader.latest().is_some() {
                task.send();
            }
        }
        *self.shared.updates.borrow_mut() = Some(reader);
    }

    /// `getRenderer()`: runs `f` with the renderer (C++ returns a pointer).
    pub fn with_renderer<R>(&self, f: impl FnOnce(&mut Renderer) -> R) -> R {
        f(&mut self.shared.renderer())
    }

    /// `getBackend()`.
    pub fn get_backend(&self) -> &dyn RendererBackend {
        self.shared.backend.get_renderer_backend()
    }

    /// `getCameraOptions()`.
    pub fn get_camera_options(&self) -> CameraOptions {
        match self.shared.latest_parameters().as_ref() {
            // RendererState::getCameraOptions(updateParameters, padding = {})
            Some(parameters) => {
                parameters.transform_state.get_camera_options(Some(mln_util::geo::EdgeInsets::default()))
            }
            None => CameraOptions::default(),
        }
    }

    /// `hasLayer(id)` (RendererState::hasLayer).
    pub fn has_layer(&self, id: &str) -> bool {
        match self.shared.latest_parameters().as_ref() {
            Some(parameters) => parameters.layers.iter().any(|layer| layer.id == id),
            None => false,
        }
    }

    /// `hasSource(id)` (RendererState::hasSource).
    pub fn has_source(&self, id: &str) -> bool {
        match self.shared.latest_parameters().as_ref() {
            Some(parameters) => parameters.sources.iter().any(|source| source.base().id == id),
            None => false,
        }
    }

    /// `hasImage(id)` (RendererState::hasImage).
    pub fn has_image(&self, id: &str) -> bool {
        match self.shared.latest_parameters().as_ref() {
            Some(parameters) => parameters.images.iter().any(|image| image.id == id),
            None => false,
        }
    }

    // gtests-core: pixelForLatLng/latLngForPixel (test/map Map.RendererState),
    // with RendererState::pixelForLatLng/latLngForPixel inlined.
    /// `pixelForLatLng(coordinate)`.
    pub fn pixel_for_lat_lng(&self, coordinate: &mln_util::geo::LatLng) -> mln_util::geo::ScreenCoordinate {
        match self.shared.latest_parameters().as_ref() {
            Some(parameters) => {
                let state = &parameters.transform_state;
                // C++ computes the unwrapped LatLng but projects `latLng`
                // (renderer_state.cpp); kept.
                let mut unwrapped_lat_lng = coordinate.wrapped();
                unwrapped_lat_lng.unwrap_for_shortest_path(&state.get_lat_lng(mln_util::geo::WrapMode::Wrapped));
                let _ = unwrapped_lat_lng;
                let point = state.lat_lng_to_screen_coordinate(coordinate);
                mln_util::geo::ScreenCoordinate::new(point.x, f64::from(state.get_size().height) - point.y)
            }
            None => mln_util::geo::ScreenCoordinate::default(),
        }
    }

    /// `latLngForPixel(point)`. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or
    /// infinite point.
    pub fn lat_lng_for_pixel(&self, point: &mln_util::geo::ScreenCoordinate) -> Result<mln_util::geo::LatLng, Error> {
        match self.shared.latest_parameters().as_ref() {
            Some(parameters) => {
                let state = &parameters.transform_state;
                let mut flipped_point = *point;
                flipped_point.y = f64::from(state.get_size().height) - flipped_point.y;
                state.screen_coordinate_to_lat_lng(&flipped_point, mln_util::geo::WrapMode::Wrapped)
            }
            None => Ok(mln_util::geo::LatLng::default()),
        }
    }

    pub fn read_still_image(&self) -> PremultipliedImage {
        self.shared.backend.read_still_image()
    }

    /// `render(Map&)`: renders a still image, or fails with the map's still
    /// image error. The map handles its renderer's events meanwhile, and
    /// keeps its own events for its owner.
    pub fn render(&mut self, map: &mut Map) -> Result<RenderResult, Error> {
        self.attach(map);
        let backend = self.shared.backend.clone();
        let _guard = BackendScope::new(backend.get_renderer_backend(), ScopeType::Explicit);

        let outcome: Rc<RefCell<Option<Result<(), Error>>>> = Rc::new(RefCell::new(None));
        let o = outcome.clone();
        map.render_still(Box::new(move |result| *o.borrow_mut() = Some(result)));

        // C++ spins on runOnce, which costs a core per renderer while the
        // workers parse tiles. Sleeping as soon as the loop is idle delays
        // the short waits of a single renderer; spinning briefly keeps them.
        const SPIN: std::time::Duration = std::time::Duration::from_millis(2);
        let run_loop = RunLoop::get();
        let mut idle_since = mln_util::time::Instant::now();
        let mut image = PremultipliedImage::new(Size::default());
        loop {
            map.process_events();
            // C++ reads the image inside the callback, which runs at the end
            // of the frame; nothing renders in between.
            match outcome.borrow_mut().take() {
                Some(Ok(())) => image = backend.read_still_image(),
                Some(Err(e)) => return Err(e),
                None => {}
            }
            if image.valid() {
                break;
            }
            if run_loop.run_once() {
                idle_since = mln_util::time::Instant::now();
            } else if idle_since.elapsed() >= SPIN {
                run_loop.wait_for_work();
            }
        }

        let stats = backend.get_renderer_backend().get_context().rendering_stats().clone();
        Ok(RenderResult { image, stats })
    }

    /// `renderOnce(Map&)`.
    pub fn render_once(&mut self, map: &mut Map) {
        self.attach(map);
        RunLoop::get().run_once();
        map.process_events();
    }

    pub fn render_frame(&self) {
        Shared::render_frame(&self.shared);
    }

    pub fn get_transform_state(&self) -> Option<TransformState> {
        self.shared.latest_parameters().map(|p| p.transform_state.clone())
    }
}

impl Shared {
    fn renderer(&self) -> RefMut<'_, Renderer> {
        RefMut::map(self.renderer.borrow_mut(), |renderer| {
            renderer.get_or_insert_with(|| {
                Renderer::with_glyph_cache(
                    self.renderer_backend.clone(),
                    self.pixel_ratio,
                    None,
                    self.glyph_cache.clone(),
                )
            })
        })
    }

    /// The map published an update or went.
    fn on_wake(shared: &Rc<Shared>) {
        let gone = shared.updates.borrow().as_ref().is_some_and(UpdateReader::is_closed);
        if gone {
            shared.updates.borrow_mut().take();
            let renderer = shared.renderer.borrow_mut().take();
            drop(renderer);
        } else if shared.invalidate_on_update {
            Shared::render_frame(shared);
        }
    }

    /// The map's latest update, once the frontend is attached to a map.
    fn latest_parameters(&self) -> Option<Arc<UpdateParameters>> {
        Some(self.updates.borrow().as_ref()?.latest()?.parameters)
    }

    fn render_frame(shared: &Rc<Shared>) {
        if let Some(update_parameters) = shared.latest_parameters() {
            let start_time = mln_util::time::Instant::now();
            let backend = shared.backend.clone();
            let _guard = BackendScope::new(backend.get_renderer_backend(), ScopeType::Explicit);
            shared.renderer().render(&update_parameters);
            shared.frame_time.set(start_time.elapsed().as_secs_f64());
        }
    }
}

#[cfg(test)]
#[path = "headless_frontend_test.rs"]
mod headless_frontend_test;
