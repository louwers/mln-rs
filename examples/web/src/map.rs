//! The map in a Web Worker of its own: a continuous map in the page's canvas
//! (an `OffscreenCanvas`), its tile workers in other Web Workers, moved by the
//! page's input through a gesture recognizer (`mln_gestures`).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_actor::RunLoop;
use mln_actor::cooperative_scheduler::CooperativeScheduler;
use mln_gestures::{Button, GestureRecognizer, Input, Key, Modifiers, PointerKind, WheelDelta};
use mln_gfx::{BackendScope, FrameTarget, RendererBackend, ScopeType};
use mln_map::{
    AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, Map, MapEvent, MapOptions,
    UpdateReader,
};
use mln_renderer::Renderer;
use mln_renderer::text::font_face_engine;
use mln_util::LatLng;
use mln_util::geo::ScreenCoordinate;
use mln_util::size::Size;
use mln_web_host::shaper::{self, ShaperModuleEngine};
use mln_web_host::{install_cooperative_scheduler, pump, tile_workers};
use wasm_bindgen::prelude::*;

#[cfg(feature = "webgl")]
use self::backend::Readback;
use self::backend::Target;
use crate::{deadline, js_error};

/// Draws the map's updates on the canvas.
struct Frontend {
    /// Dropped first, before its backend.
    renderer: Renderer,
    target: Target,
    backend: Rc<dyn RendererBackend>,
    updates: UpdateReader,
    /// The generation of the update the renderer drew last.
    drawn_update: u64,
    /// Draw the latest update again, though the renderer drew it.
    redraw: bool,
}

impl Frontend {
    /// Draws the latest update if it is new; returns whether it drew.
    fn render(&mut self) -> bool {
        let Some(update) = self.updates.latest() else {
            return false;
        };
        if !self.redraw && update.generation == self.drawn_update {
            return false;
        }
        let _scope = BackendScope::new(&*self.backend, ScopeType::Implicit);
        if self.target.begin_frame().is_none() {
            return false;
        }
        self.redraw = false;
        self.drawn_update = update.generation;
        self.renderer.render(&update.parameters, &self.target);
        true
    }
}

fn physical(size: Size, pixel_ratio: f64) -> Size {
    Size::new(
        (f64::from(size.width) * pixel_ratio).round().max(1.0) as u32,
        (f64::from(size.height) * pixel_ratio).round().max(1.0) as u32,
    )
}

/// Hands this instance's fetches the responses its Web Worker fetched ahead
/// (web/map-worker.ts): `take(url)` returns a promise of the response
/// fetched for exactly `url`, once, or undefined (`mln_storage`'s
/// `set_fetch_preloads`).
#[wasm_bindgen]
pub fn set_preloads(take: js_sys::Function) {
    mln_storage::set_fetch_preloads(Some(take));
}

/// A continuous map in an `OffscreenCanvas`, in the Web Worker that runs it
/// (web/map-worker.ts).
#[wasm_bindgen]
pub struct WebMap {
    // Dropped first, before the map.
    frontend: Frontend,
    map: Map,
    /// The map finished loading its style.
    loaded: bool,
    run_loop: RunLoop,
    scheduler: Arc<CooperativeScheduler>,
    /// Runs the `RunLoop` between frames.
    _between_frames: pump::Step,
    pixel_ratio: f64,
    received: Rc<Cell<u32>>,
    /// The page's pointer, wheel and key input, turned into camera moves.
    gestures: GestureRecognizer,
}

#[wasm_bindgen]
impl WebMap {
    /// Creates the WebGPU device, a surface on `canvas` (the page's canvas,
    /// transferred to this worker) and the map, at `width` x `height` CSS
    /// pixels. The tiles' workers run in the Web Workers at the other end of
    /// `ports` (at least one), and `on_fatal(instance, message)` is called if
    /// one of them panics. Between frames, `mln_web_host::pump` runs the
    /// map's work (a fetch finished, a reply arrived, a task was queued) as
    /// it arrives. `load_shaper()` returns a promise of the shaper module
    /// (`web/pkg/mln_web_shaper.js`), initialized; the map calls it the first
    /// time a style declares font faces. With `local_ideograph_font_family`
    /// (a CSS font-family list), the map draws CJK glyphs in it rather than
    /// downloading them, as GL JS's `localIdeographFontFamily`.
    /// (`map_webgl.rs` has the `webgl` build's.)
    #[cfg(not(feature = "webgl"))]
    #[allow(clippy::too_many_arguments)]
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(js_error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| js_error(format!("WebGPU found no adapter: {e}")))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("MapLibre WebGPU Device"),
                required_limits: wgpu::Limits::default().or_worse_values_from(&adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| js_error(format!("WebGPU device: {e}")))?;
        // Dawn's logUncapturedError instead of wgpu's panicking default.
        device.on_uncaptured_error(Arc::new(|error: wgpu::Error| {
            mln_util::logging::Log::error(
                mln_util::logging::Event::Render,
                format!("WebGPU validation error: {error}"),
            );
        }));
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.first().copied().unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
        // A WebGPU canvas takes any texture usage, so its frames can be read
        // back (wgpu reports only RENDER_ATTACHMENT for it).
        let options = mln_webgpu::SurfaceOptions {
            cap_frame_rate: true,
            usages: Some(capabilities.usages | wgpu::TextureUsages::COPY_SRC),
            format: None,
        };
        let device = mln_webgpu::Device::from_wgpu(Some(instance), device, queue, Some(format));
        let webgpu_backend = Rc::new(mln_webgpu::RendererBackend::new(&device));
        let target = Target::new(webgpu_backend.clone(), surface, physical(size, pixel_ratio), options)
            .map_err(|e| js_error(format!("WebGPU surface: {e}")))?;
        WebMap::build(
            webgpu_backend,
            target,
            size,
            pixel_ratio,
            &ports,
            on_fatal,
            load_shaper,
            local_ideograph_font_family,
        )
    }

    /// Sets the font faces that every font stack of the styles loaded from
    /// now on falls back to: `json` is an array of font files, each an
    /// object with a `url` and an optional `unicode-range`, as in a font
    /// face of a style's `font-faces`.
    pub fn set_fallback_font_faces(&mut self, json: &str) -> Result<(), JsValue> {
        self.map.style_mut().set_fallback_font_faces(json).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Loads a style from a URL.
    pub fn set_style_url(&mut self, url: &str) {
        self.map.style_mut().load_url(url);
    }

    /// Moves the camera.
    pub fn jump_to(&mut self, latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) {
        let center = LatLng::new(latitude, longitude).ok();
        self.map.jump_to(&CameraOptions {
            center,
            zoom: Some(zoom),
            bearing: Some(bearing),
            pitch: Some(pitch),
            ..Default::default()
        });
    }

    /// The camera as `[latitude, longitude, zoom, bearing, pitch]`, in
    /// degrees but for the zoom.
    pub fn camera(&self) -> Vec<f64> {
        let camera = self.map.get_camera_options(None);
        let center = camera.center.unwrap_or_default();
        vec![
            center.latitude(),
            center.longitude(),
            camera.zoom.unwrap_or_default(),
            camera.bearing.unwrap_or_default(),
            camera.pitch.unwrap_or_default(),
        ]
    }

    /// Eases the bearing and pitch back to zero, as a step of the page's
    /// input.
    pub fn reset_north(&mut self) {
        let update = CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions { bearing: Some(0.0), pitch: Some(0.0), ..Default::default() },
            animation: AnimationOptions::new(Duration::from_millis(500)),
            phase: GesturePhase::None,
        };
        self.gestures.step_to(&mut self.map, update);
    }

    /// Lets drags and zooms on the globe turn the bearing with the planet,
    /// so that a drag carries on over a pole (`Map::set_globe_free_bearing`).
    /// Off, as the map starts, they keep north up.
    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.map.set_globe_free_bearing(free_bearing);
    }

    /// Jumps to another map's camera, for a comparison that this map
    /// follows: its own motion stops (an ease, momentum, a wheel's eased
    /// zoom).
    pub fn follow(&mut self, latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) {
        self.map.cancel_transitions();
        let update = CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: CameraOptions {
                center: LatLng::new(latitude, longitude).ok(),
                zoom: Some(zoom),
                bearing: Some(bearing),
                pitch: Some(pitch),
                ..Default::default()
            },
            animation: AnimationOptions::default(),
            phase: GesturePhase::None,
        };
        self.gestures.step_to(&mut self.map, update);
    }

    /// The attributions of the style's sources that have one, as HTML. A
    /// source that links a TileJSON has one once the TileJSON loads.
    pub fn attributions(&self) -> Vec<String> {
        let style = self.map.style();
        style.get_sources().iter().filter_map(|source| source.get_attribution()).filter(|a| !a.is_empty()).collect()
    }

    /// The style's `name`, or an empty string.
    pub fn style_name(&self) -> String {
        self.map.style().get_name()
    }

    /// Resizes the map and its surface to `width` x `height` CSS pixels.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        let size = Size::new(width.max(1), height.max(1));
        self.frontend.target.resize(physical(size, self.pixel_ratio));
        self.map.set_size(size).map_err(js_error)
    }

    /// Draws the map at `pixel_ratio` device pixels per CSS pixel from now
    /// on, as on a display of another density, keeping its tiles, and sizes
    /// the surface for it.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f64) -> Result<(), JsValue> {
        self.map.set_pixel_ratio(pixel_ratio as f32).map_err(js_error)?;
        self.pixel_ratio = pixel_ratio;
        self.frontend.renderer.set_pixel_ratio(pixel_ratio as f32);
        let size = self.map.get_map_options().size;
        self.frontend.target.resize(physical(size, pixel_ratio));
        Ok(())
    }

    /// How many worker replies arrived so far.
    pub fn received(&self) -> u32 {
        self.received.get()
    }

    /// One animation frame at `now` (milliseconds, the clock of the input
    /// methods' `time`): samples the map's animations at this frame, steps
    /// the gestures' eased zoom and ends those that time out, runs the `RunLoop`'s
    /// tasks, draws the map's latest update if it is new, then runs the
    /// cooperative scheduler's tasks (sprites, glyphs, raster tiles) for up
    /// to `budget_ms` and delivers their results. Returns whether it drew.
    pub fn frame(&mut self, budget_ms: f64, now: f64) -> bool {
        // The map samples its animations at this frame's time.
        self.map.set_frame_time(Some(mln_util::time::Instant::now()));
        self.gestures.frame(&mut self.map, now);
        self.run_loop.run_once();
        self.handle_map_events();
        let drew = self.frontend.render();
        self.run_tasks(budget_ms);
        self.handle_map_events();
        drew
    }

    /// A pointer went down on the canvas, at `x`, `y` CSS pixels from its
    /// top-left corner: `kind` is the DOM `pointerType` ("mouse", "touch" or
    /// "pen"), `button` the DOM `button` (0 primary, 1 middle, 2 secondary)
    /// and `modifiers` the bits of [`modifiers`]. `time` is in milliseconds
    /// on the clock `frame` gets. Returns whether the map used it.
    #[allow(clippy::too_many_arguments)]
    pub fn pointer_down(
        &mut self,
        id: u32,
        kind: &str,
        button: i32,
        x: f64,
        y: f64,
        modifiers: u32,
        time: f64,
    ) -> bool {
        let kind = match kind {
            "touch" => PointerKind::Touch,
            "pen" => PointerKind::Pen,
            _ => PointerKind::Mouse,
        };
        let button = match button {
            1 => Button::Middle,
            2 => Button::Secondary,
            _ => Button::Primary,
        };
        let input = Input::PointerDown {
            id,
            kind,
            button,
            position: ScreenCoordinate::new(x, y),
            modifiers: modifiers_from_bits(modifiers),
        };
        self.gestures.handle(&mut self.map, input, time)
    }

    /// A pointer moved (see `pointer_down`).
    pub fn pointer_move(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.gestures.handle(&mut self.map, Input::PointerMove { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// A pointer went up (see `pointer_down`).
    pub fn pointer_up(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.gestures.handle(&mut self.map, Input::PointerUp { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// The browser took a pointer away (`pointercancel`).
    pub fn pointer_cancel(&mut self, id: u32, time: f64) -> bool {
        self.gestures.handle(&mut self.map, Input::PointerCancel { id }, time)
    }

    /// A DOM `wheel` event, as the page classifies it (web/main.ts): at
    /// (`x`, `y`), `amount` notches of a mouse wheel when `kind` is
    /// [`WHEEL_NOTCHES`], or `amount` pixels of continuous scrolling when it
    /// is [`WHEEL_PIXELS`]; positive scrolls down.
    pub fn wheel(&mut self, x: f64, y: f64, kind: u32, amount: f64, modifiers: u32, time: f64) -> bool {
        debug_assert!(kind == WHEEL_NOTCHES || kind == WHEEL_PIXELS, "wheel kind {kind}");
        let delta = if kind == WHEEL_NOTCHES { WheelDelta::Notches(amount) } else { WheelDelta::Pixels(amount) };
        let input =
            Input::Wheel { position: ScreenCoordinate::new(x, y), delta, modifiers: modifiers_from_bits(modifiers) };
        self.gestures.handle(&mut self.map, input, time)
    }

    /// A DOM `keydown`, by its `key`: the arrows, `+`/`=`, `-`/`_` and
    /// Escape; the map ignores the others and returns false.
    pub fn key(&mut self, key: &str, modifiers: u32, time: f64) -> bool {
        let key = match key {
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "+" | "=" => Key::ZoomIn,
            "-" | "_" => Key::ZoomOut,
            "Escape" => Key::Escape,
            _ => return false,
        };
        self.gestures.handle(&mut self.map, Input::Key { key, modifiers: modifiers_from_bits(modifiers) }, time)
    }

    /// The canvas lost focus: every pointer is released.
    pub fn blur(&mut self, time: f64) -> bool {
        self.gestures.handle(&mut self.map, Input::Blur, time)
    }

    /// The box a Shift-drag zooms to while it is drawn, as `[x0, y0, x1,
    /// y1]` in CSS pixels, for the page to show; otherwise undefined.
    pub fn box_zoom(&self) -> Option<Vec<f64>> {
        self.gestures.box_zoom().map(|[a, b]| vec![a.x, a.y, b.x, b.y])
    }

    /// Whether the style loaded and every tile and resource the map needs
    /// is loaded.
    pub fn is_fully_loaded(&self) -> bool {
        self.loaded && self.map.is_fully_loaded()
    }

    /// Draws the map's latest update again with the next frame and copies
    /// it (see `take_pixels`).
    pub fn request_pixels(&mut self) {
        self.frontend.target.request_readback();
        self.frontend.redraw = true;
    }

    /// The frame `request_pixels` asked for, as RGBA rows of the canvas's
    /// physical size, once the GPU has copied it; until then undefined.
    pub fn take_pixels(&self) -> Option<Vec<u8>> {
        self.frontend.target.take_readback().map(|image| image.data)
    }
}

/// `WebMap::wheel`'s `kind`, as web/protocol.ts's `WHEEL_PIXELS` and
/// `WHEEL_NOTCHES` send it.
const WHEEL_PIXELS: u32 = 0;
const WHEEL_NOTCHES: u32 = 1;

/// The input methods' `modifiers`: Shift 1, Ctrl 2, Alt 4, Meta 8.
fn modifiers_from_bits(bits: u32) -> Modifiers {
    Modifiers { shift: bits & 1 != 0, ctrl: bits & 2 != 0, alt: bits & 4 != 0, meta: bits & 8 != 0 }
}

impl WebMap {
    /// The map on `backend`, drawing into `target`: what both builds'
    /// `create` share (see there).
    #[allow(clippy::too_many_arguments)]
    fn build(
        backend: Rc<dyn RendererBackend>,
        target: Target,
        size: Size,
        pixel_ratio: f64,
        ports: &js_sys::Array,
        on_fatal: js_sys::Function,
        load_shaper: js_sys::Function,
        local_ideograph_font_family: Option<String>,
    ) -> Result<WebMap, JsValue> {
        let scheduler = install_cooperative_scheduler();
        shaper::set_loader(load_shaper);
        font_face_engine::set_font_face_engine(Arc::new(ShaperModuleEngine));

        let run_loop = RunLoop::get();
        run_loop.set_waker(Some(pump::waker()));
        scheduler.set_waker(Some(pump::waker()));
        let between_frames = {
            let run_loop = run_loop.clone();
            pump::add(move || {
                run_loop.run_once();
                true
            })
        };
        let mut renderer = Renderer::new(backend.clone(), pixel_ratio as f32, local_ideograph_font_family);
        let received = Rc::new(Cell::new(0));
        let count = received.clone();
        let transport = tile_workers::transport(
            ports.iter().map(JsCast::unchecked_into).collect(),
            move |_| count.set(count.get() + 1),
            move |instance, message| {
                let _ = on_fatal.call2(&JsValue::NULL, &JsValue::from(instance as u32), &message);
            },
        );
        renderer.set_tile_worker_transport(transport);
        let mut map =
            Map::new(MapOptions { size, pixel_ratio: pixel_ratio as f32, ..Default::default() }).map_err(js_error)?;
        map.set_waker(Some(pump::waker()));
        renderer.set_event_queue(Some(map.renderer_events()));
        let frontend =
            Frontend { renderer, target, backend, updates: map.update_reader(), drawn_update: 0, redraw: false };
        Ok(WebMap {
            frontend,
            map,
            loaded: false,
            run_loop,
            scheduler,
            _between_frames: between_frames,
            pixel_ratio,
            received,
            gestures: GestureRecognizer::default(),
        })
    }

    /// Lets the map handle what its renderer and its style reported, and
    /// notes what the page asks about.
    fn handle_map_events(&mut self) {
        for event in self.map.drain_events() {
            match event {
                MapEvent::LoadingFailed(error) => {
                    web_sys::console::error_1(&JsValue::from_str(&format!("map failed to load: {error:?}")));
                }
                MapEvent::LoadingFinished => self.loaded = true,
                _ => {}
            }
        }
    }

    /// Runs the cooperative scheduler's tasks for up to `budget_ms`, then the
    /// `RunLoop`'s for what they delivered. Returns whether tasks remain.
    fn run_tasks(&self, budget_ms: f64) -> bool {
        let more = self.scheduler.run_until(Some(deadline(budget_ms)));
        self.run_loop.run_once();
        more
    }
}

/// What draws the map: the browser's WebGPU through wgpu, or, in the `webgl`
/// build of the module, its WebGL2 through `mln_gl` (`map_webgl.rs`).
#[cfg(not(feature = "webgl"))]
mod backend {
    pub(super) use mln_webgpu::SurfaceTarget as Target;
}
#[cfg(feature = "webgl")]
#[path = "map_webgl.rs"]
mod backend;
