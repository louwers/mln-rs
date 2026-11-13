// C++ renders with the one backend it was built with. The frontend renders
// on the device it is given, or on WebGPU, or on the backend the
// environment variable MLN_RENDER_BACKEND names for the test harness:
// `webgpu`, `metal`, `vulkan` or `gl`.

use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_actor::{AsyncTask, RunLoop};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::{BackendScope, RendererBackend, ScopeType};
use mln_map::Map;
use mln_renderer::{Renderer, UpdateParameters, UpdateReader};
use mln_runtime::{BackendType, Device, DeviceOptions, GlApi, LocalTarget};
use mln_text::GlyphCache;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

pub struct RenderResult {
    pub image: PremultipliedImage,
    pub stats: RenderingStats,
}

struct Shared {
    size: Cell<Size>,
    pixel_ratio: Cell<f32>,
    target: LocalTarget,
    /// The target's backend, which the renderer draws with.
    backend: Rc<dyn RendererBackend>,
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

/// The device a frontend renders on without one of its own: on the backend
/// `MLN_RENDER_BACKEND` names, or on WebGPU. `gl` names OpenGL ES, and
/// `gl-desktop` desktop OpenGL.
pub fn harness_device() -> Result<Device, Error> {
    let mut options = DeviceOptions::default();
    let backend = match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("gl") => BackendType::OpenGL,
        Ok("gl-desktop") => {
            options.gl_api = GlApi::Gl;
            BackendType::OpenGL
        }
        Ok("webgpu") | Err(_) => BackendType::WebGPU,
        Ok(other) => return Err(Error::invalid_argument(format!("MLN_RENDER_BACKEND names no backend: {other}"))),
    };
    Device::with_backend(backend, options)
}

fn physical_size(size: Size, pixel_ratio: f32) -> Size {
    Size::new((size.width as f32 * pixel_ratio) as u32, (size.height as f32 * pixel_ratio) as u32)
}

impl HeadlessFrontend {
    /// A frontend that invalidates on update, on [`harness_device`].
    ///
    /// Fails as [`harness_device`] does, and with the backend's error when
    /// it can't render on the device.
    pub fn new(size: Size, pixel_ratio: f32) -> Result<Self, Error> {
        Self::with_options(size, pixel_ratio, true)
    }

    /// A frontend whose renderer has a glyph cache of its own.
    pub fn with_options(size: Size, pixel_ratio: f32, invalidate_on_update: bool) -> Result<Self, Error> {
        Self::with_glyph_cache(size, pixel_ratio, invalidate_on_update, Arc::default())
    }

    /// A frontend whose renderer uses `glyph_cache`. Frontends given the same
    /// `GlyphCache` share the glyphs their renderers parse from glyph PBFs,
    /// also across threads.
    pub fn with_glyph_cache(
        size: Size,
        pixel_ratio: f32,
        invalidate_on_update: bool,
        glyph_cache: Arc<GlyphCache>,
    ) -> Result<Self, Error> {
        Self::with_device(harness_device()?, size, pixel_ratio, invalidate_on_update, glyph_cache)
    }

    /// Port-only: a frontend that renders on `device`, such as one that other
    /// frontends or the host share.
    ///
    /// Fails with the backend's error when it can't render on the device.
    pub fn with_device(
        device: Device,
        size: Size,
        pixel_ratio: f32,
        invalidate_on_update: bool,
        glyph_cache: Arc<GlyphCache>,
    ) -> Result<Self, Error> {
        let target = LocalTarget::offscreen(Some(device), physical_size(size, pixel_ratio))?;
        let backend = target.backend();
        let shared = Rc::new(Shared {
            size: Cell::new(size),
            pixel_ratio: Cell::new(pixel_ratio),
            target,
            backend,
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
        Ok(HeadlessFrontend { shared })
    }

    /// A 256x256 frontend.
    pub fn with_pixel_ratio(pixel_ratio: f32) -> Result<Self, Error> {
        Self::new(Size::new(256, 256), pixel_ratio)
    }

    #[must_use]
    pub fn get_size(&self) -> Size {
        self.shared.size.get()
    }

    pub fn set_size(&mut self, size: Size) {
        if self.shared.size.get() != size {
            self.shared.size.set(size);
            self.shared.resize_target();
        }
    }

    /// Renders at `pixel_ratio` from now on, which should be the map's
    /// (`Map::set_pixel_ratio`): sizes the backend for it, and sets the
    /// renderer's.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if self.shared.pixel_ratio.get() != pixel_ratio {
            self.shared.pixel_ratio.set(pixel_ratio);
            self.shared.resize_target();
            if let Some(renderer) = self.shared.renderer.borrow_mut().as_mut() {
                renderer.set_pixel_ratio(pixel_ratio);
            }
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

    /// Runs `f` with the renderer.
    pub fn with_renderer<R>(&self, f: impl FnOnce(&mut Renderer) -> R) -> R {
        f(&mut self.shared.renderer())
    }

    #[must_use]
    pub fn get_backend(&self) -> &dyn RendererBackend {
        &*self.shared.backend
    }

    /// The device the frontend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        self.shared.target.device()
    }

    /// The last frame's pixels, or an empty image without one.
    #[must_use]
    pub fn read_still_image(&self) -> PremultipliedImage {
        self.shared.read_image().unwrap_or_else(|_| PremultipliedImage::new(Size::default()))
    }

    /// `render(Map&)`: renders a still image, or fails with the map's still
    /// image error. The map handles its renderer's events meanwhile, and
    /// keeps its own events for its owner.
    pub fn render(&mut self, map: &mut Map) -> Result<RenderResult, Error> {
        // C++ spins on runOnce, which costs a core per renderer while the
        // workers parse tiles. Sleeping as soon as the loop is idle delays
        // the short waits of a single renderer; spinning briefly keeps them.
        const SPIN: std::time::Duration = std::time::Duration::from_millis(2);

        let backend = Rc::clone(&self.shared.backend);
        self.attach(map);
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);

        let outcome: Rc<RefCell<Option<Result<(), Error>>>> = Rc::new(RefCell::new(None));
        let o = Rc::clone(&outcome);
        map.render_still(Box::new(move |result| *o.borrow_mut() = Some(result)));

        let run_loop = RunLoop::get();
        let mut idle_since = mln_util::time::Instant::now();
        let mut image = PremultipliedImage::new(Size::default());
        loop {
            map.process_events();
            // C++ reads the image inside the callback, which runs at the end
            // of the frame; nothing renders in between.
            match outcome.borrow_mut().take() {
                Some(Ok(())) => image = self.shared.read_image()?,
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

        let stats = backend.get_context().rendering_stats().clone();
        Ok(RenderResult { image, stats })
    }

    pub fn render_frame(&self) {
        Shared::render_frame(&self.shared);
    }

    /// Port-only: the target the frontend draws into, so a host can reach
    /// the frame's colour and depth-stencil textures through
    /// [`LocalTarget::frames`] and draw into them alongside the map.
    #[must_use]
    pub fn target(&self) -> &LocalTarget {
        &self.shared.target
    }
}

impl Shared {
    fn renderer(&self) -> RefMut<'_, Renderer> {
        RefMut::map(self.renderer.borrow_mut(), |renderer| {
            renderer.get_or_insert_with(|| {
                Renderer::with_glyph_cache(
                    Rc::clone(&self.backend),
                    self.pixel_ratio.get(),
                    None,
                    Arc::clone(&self.glyph_cache),
                )
            })
        })
    }

    /// Sizes the target for the frontend's size and pixel ratio.
    fn resize_target(&self) {
        self.target.resize(physical_size(self.size.get(), self.pixel_ratio.get()));
    }

    /// The last frame's pixels.
    fn read_image(&self) -> Result<PremultipliedImage, Error> {
        let _guard = BackendScope::new(&*self.backend, ScopeType::Explicit);
        self.target.read_image()
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
            Shared::render(shared, &update_parameters);
        }
    }

    fn render(shared: &Rc<Shared>, update_parameters: &Arc<UpdateParameters>) {
        let backend = Rc::clone(&shared.backend);
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
        if let Some(frames) = shared.target.frames() {
            shared.renderer().render(update_parameters, frames);
        }
    }
}

#[cfg(test)]
#[path = "headless_frontend_test.rs"]
mod headless_frontend_test;
