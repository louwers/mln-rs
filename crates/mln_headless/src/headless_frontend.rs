//! Rendering without a window: [`HeadlessFrontend`] draws a map into an
//! offscreen texture on the calling thread, for the render tests, the tests
//! that render a map, and the benchmarks.

// C++ renders with the one backend it was built with. The frontend renders
// on the device it is given, or on WebGPU, or on the backend the
// environment variable MLN_RENDER_BACKEND names for the test harness:
// `webgpu`, `metal`, `vulkan`, `d3d12` or `gl`.

use std::sync::Arc;

use mln_gfx::rendering_stats::RenderingStats;
use mln_map::Map;
use mln_renderer::{Renderer, RendererEvent, RendererEvents, UpdateParameters, UpdateReader};
use mln_runtime::{BackendRef, BackendType, Device, DeviceOptions, GlApi, LocalTarget};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::size::Size;
use mln_text::GlyphCache;
use mln_text::local_fonts::LocalGlyphs;
use mln_util::event_queue::Waker;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use parking_lot::Mutex;

/// A still image, and the stats of the renderer that drew it.
pub struct RenderResult {
    pub image: PremultipliedImage,
    pub stats: RenderingStats,
}

/// Renders a map into an offscreen texture on the calling thread.
///
/// The frontend runs the queue of the calling thread while it waits for a
/// still image, and runs its renderer's background work on a pool. It draws
/// the updates of the map it is attached to ([`attach`](Self::attach), which
/// [`render`](Self::render) does too) when its owner asks: [`render`](Self::render) draws until a still image
/// is complete, [`render_update`](Self::render_update) draws an update it
/// hasn't drawn yet, and [`render_frame`](Self::render_frame) draws the
/// latest one again. Once the map is gone, `render_update` tears the
/// renderer down, and the frontend makes another when next used.
pub struct HeadlessFrontend {
    size: Size,
    pixel_ratio: f32,
    target: LocalTarget,
    glyph_cache: Arc<GlyphCache>,
    /// The local fonts the renderer draws from: none unless set.
    local_glyphs: LocalGlyphs,
    /// The calling thread's queue, which [`render`](Self::render) runs.
    queue: Queue<()>,
    /// Where the renderer's background work runs.
    pool: Pool,
    /// The queue that the attached map's renderer reports to, where the
    /// renderer's waker asks for an update.
    reports_to: Arc<Mutex<Option<RendererEvents>>>,
    /// The stats that the last renderer left as it went.
    stats: RenderingStats,
    /// The updates of the map that the frontend renders.
    updates: Option<UpdateReader>,
    /// The generation of the last update drawn from `updates`, or 0.
    drawn: u64,
}

/// The device a frontend renders on without one of its own: on the backend
/// `MLN_RENDER_BACKEND` names, or on WebGPU. `gl` names OpenGL ES, and
/// `gl-desktop` desktop OpenGL.
pub fn harness_device() -> Result<Device, Error> {
    let mut options = DeviceOptions::default();
    let backend = match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("d3d12") => BackendType::D3D12,
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
    /// A frontend on [`harness_device`] whose renderer has a glyph cache of
    /// its own.
    ///
    /// Fails as [`harness_device`] does, and with the backend's error when
    /// it can't render on the device.
    pub fn new(size: Size, pixel_ratio: f32, queue: &Queue<()>, pool: &Pool) -> Result<Self, Error> {
        Self::with_glyph_cache(size, pixel_ratio, Arc::default(), queue, pool)
    }

    /// A 256x256 frontend.
    pub fn with_pixel_ratio(pixel_ratio: f32, queue: &Queue<()>, pool: &Pool) -> Result<Self, Error> {
        Self::new(Size::new(256, 256), pixel_ratio, queue, pool)
    }

    /// A frontend whose renderer uses `glyph_cache`. Frontends given the same
    /// `GlyphCache` share the glyphs their renderers parse from glyph PBFs,
    /// also across threads.
    pub fn with_glyph_cache(
        size: Size,
        pixel_ratio: f32,
        glyph_cache: Arc<GlyphCache>,
        queue: &Queue<()>,
        pool: &Pool,
    ) -> Result<Self, Error> {
        Self::with_device(harness_device()?, size, pixel_ratio, glyph_cache, queue, pool)
    }

    /// A frontend that renders on `device`, such as one that other frontends
    /// or the host share.
    ///
    /// Fails with the backend's error when it can't render on the device.
    pub fn with_device(
        device: Device,
        size: Size,
        pixel_ratio: f32,
        glyph_cache: Arc<GlyphCache>,
        queue: &Queue<()>,
        pool: &Pool,
    ) -> Result<Self, Error> {
        let target = LocalTarget::offscreen(Some(device), physical_size(size, pixel_ratio))?;
        Ok(HeadlessFrontend {
            size,
            pixel_ratio,
            target,
            glyph_cache,
            local_glyphs: LocalGlyphs::default(),
            queue: queue.clone(),
            pool: pool.clone(),
            reports_to: Arc::default(),
            stats: RenderingStats::default(),
            updates: None,
            drawn: 0,
        })
    }

    /// Draws from `local` fonts, such as the system's, rather than none:
    /// with a new renderer, which lays out the map again.
    pub fn set_local_glyphs(&mut self, local: LocalGlyphs) {
        self.local_glyphs = local;
        if let Some(stats) = self.target.tear_down_renderer() {
            self.stats = stats;
        }
        self.drawn = 0;
    }

    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn set_size(&mut self, size: Size) {
        if self.size != size {
            self.size = size;
            self.resize_target();
        }
    }

    /// Renders at `pixel_ratio` from now on, which should be the map's
    /// (`Map::set_pixel_ratio`): sizes the backend for it, and sets the
    /// renderer's.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if self.pixel_ratio != pixel_ratio {
            self.pixel_ratio = pixel_ratio;
            self.resize_target();
            if let Some(renderer) = self.target.renderer_mut() {
                renderer.set_pixel_ratio(pixel_ratio);
            }
        }
    }

    /// Renders `map` from now on: the frontend reads the map's updates, and
    /// its renderer reports to the map. It sets the map's waker to
    /// [`waker`](Self::waker), which [`render`](Self::render) needs: an owner
    /// that sets a waker of its own after must have it wake the queue too.
    /// Attaching to the map it renders already does nothing.
    pub fn attach(&mut self, map: &mut Map) {
        let reader = map.update_reader();
        if self.updates.as_ref().is_some_and(|updates| updates.ptr_eq(&reader)) {
            return;
        }
        let events = map.renderer_events();
        *self.reports_to.lock() = Some(events.clone());
        self.renderer().set_event_queue(Some(events));
        map.set_waker(Some(self.waker()));
        self.updates = Some(reader);
        self.drawn = 0;
    }

    /// Wakes the queue that [`render`](Self::render) waits on, from any
    /// thread.
    #[must_use]
    pub fn waker(&self) -> Waker {
        let queue = self.queue.downgrade();
        Arc::new(move || {
            if let Some(queue) = queue.upgrade() {
                queue.wake();
            }
        })
    }

    /// The renderer, made on first use, which reports to the attached map.
    pub fn renderer(&mut self) -> &mut Renderer {
        let HeadlessFrontend { target, pixel_ratio, glyph_cache, local_glyphs, pool, reports_to, .. } = self;
        target.renderer_or(|gpu| {
            let reports_to = Arc::clone(reports_to);
            let events = reports_to.lock().clone();
            let mut renderer = Renderer::with_glyph_cache(
                gpu,
                *pixel_ratio,
                local_glyphs.clone(),
                Arc::clone(glyph_cache),
                Arc::new(move || {
                    let events = reports_to.lock().clone();
                    if let Some(events) = events {
                        events.push(RendererEvent::Invalidate);
                    }
                }),
                pool,
            );
            renderer.set_event_queue(events);
            renderer
        })
    }

    /// Lets go of what the renderer caches and destroys what it released,
    /// as [`Renderer::reduce_memory_use`] does, if the frontend has one.
    pub fn reduce_memory_use(&mut self) {
        self.target.with_renderer(|renderer, gpu| renderer.reduce_memory_use(gpu));
    }

    /// Lets go of the renderer's data, as [`Renderer::clear_data`] does, if
    /// the frontend has a renderer.
    pub fn clear_data(&mut self) {
        self.target.with_renderer(|renderer, gpu| renderer.clear_data(gpu));
    }

    /// The backend the frontend draws with.
    #[must_use]
    pub fn backend(&self) -> BackendRef<'_> {
        self.target.backend()
    }

    /// The device the frontend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        self.target.device()
    }

    /// The last frame's pixels, or an empty image without one.
    #[must_use]
    pub fn read_still_image(&mut self) -> PremultipliedImage {
        self.target.read_image().unwrap_or_else(|_| PremultipliedImage::new(Size::default()))
    }

    /// `render(Map&)`: renders a still image, or fails with the map's still
    /// image error. It runs the frontend's queue and draws the map's updates
    /// until the image is complete, and parks on the queue while nothing
    /// happens, until the map's waker wakes it (see [`attach`](Self::attach)).
    /// The map handles its renderer's events meanwhile, and keeps its own
    /// events for its owner.
    pub fn render(&mut self, map: &mut Map) -> Result<RenderResult, Error> {
        // C++ spins on runOnce, which costs a core per renderer while the
        // workers parse tiles. Parking as soon as the queue is idle delays
        // the short waits of a single renderer; spinning briefly keeps them.
        const SPIN: std::time::Duration = std::time::Duration::from_millis(2);

        self.attach(map);
        // The request sees what the map's style reported so far, such as an
        // error that fails it.
        map.process_events();
        map.render_still()?;
        // A target that can't begin a frame won't by waiting.
        if !self.target.ready_for_frame() {
            return Err(Error::invalid_state(
                "the offscreen target has no pixels at its size and scale factor, or more than the device's textures take",
            ));
        }

        let queue = self.queue.clone();
        let mut idle_since = mln_style::time::Instant::now();
        let image = loop {
            map.process_events();
            // C++ reads the image inside the callback, which runs at the end
            // of the frame; nothing renders in between.
            if let Some(result) = map.take_still_image() {
                result?;
                break self.target.read_image()?;
            }
            // The map publishes its updates as it handles its events. The
            // queue's tasks run after each frame too, which frames that keep
            // coming would otherwise starve. The frontend parks only when
            // neither had anything to do: the map handles a frame's reports
            // first.
            let drew = self.render_update();
            if queue.run_once(&mut ()) || drew {
                idle_since = mln_style::time::Instant::now();
            } else if idle_since.elapsed() >= SPIN {
                queue.park();
            }
        };

        Ok(RenderResult { image, stats: self.rendering_stats() })
    }

    /// Draws the attached map's latest update, unless the frontend drew it
    /// already. Once the map is gone, tears the renderer down instead. Returns
    /// whether it drew.
    pub fn render_update(&mut self) -> bool {
        let Some(updates) = &self.updates else {
            return false;
        };
        if updates.is_closed() {
            self.updates = None;
            if let Some(stats) = self.target.tear_down_renderer() {
                self.stats = stats;
            }
            return false;
        }
        if updates.generation() == self.drawn {
            return false;
        }
        self.render_frame()
    }

    /// Draws the attached map's latest update, which it may have drawn
    /// already. Returns whether there was one to draw.
    pub fn render_frame(&mut self) -> bool {
        let Some(update) = self.updates.as_ref().and_then(UpdateReader::latest) else {
            return false;
        };
        self.drawn = update.generation;
        self.draw(&update.parameters);
        true
    }

    /// Draws a frame of `parameters`, such as an update of the attached map
    /// that the caller changed.
    pub fn draw(&mut self, parameters: &Arc<UpdateParameters>) {
        // Makes the renderer on first use.
        self.renderer();
        self.target.render(parameters);
    }

    /// The rendering stats of the frontend's renderer, or those the last one
    /// left as it went, with all its resources released.
    #[must_use]
    pub fn rendering_stats(&self) -> RenderingStats {
        match self.target.renderer() {
            Some(renderer) => renderer.rendering_stats(),
            None => self.stats.clone(),
        }
    }

    /// The target the frontend draws into, so a host can reach the frame's
    /// colour and depth-stencil textures and the backend's `Gpu` through
    /// [`LocalTarget::with_frame`] and draw into them alongside the map.
    pub fn target(&mut self) -> &mut LocalTarget {
        &mut self.target
    }

    /// Sizes the target for the frontend's size and pixel ratio.
    fn resize_target(&mut self) {
        self.target.resize(physical_size(self.size, self.pixel_ratio));
    }
}

#[cfg(test)]
#[path = "headless_frontend_test.rs"]
mod headless_frontend_test;
