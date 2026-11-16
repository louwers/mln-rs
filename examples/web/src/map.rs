//! The map in a Web Worker of its own: a runtime map (`mln_runtime`) whose
//! render session presents to the page's canvas (an `OffscreenCanvas`), with
//! the browser's WebGPU or, in the `webgl` build of the module, its WebGL2.
//! Its tile workers are in other Web Workers, and the page's input moves it
//! through a gesture recognizer (`mln_gestures`).
//!
//! The runtime and the session run on this Web Worker's one thread, in the
//! slices of `mln_web_host`'s pump: a command or a frame demand runs in a
//! later task, never inside the call that sends it. The page's animation
//! frames step the gestures and demand a frame when the map has something
//! new to draw, and the session renders it in a task of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_actor::LocalObject;
use mln_gestures::{Button, GestureRecognizer, Input, Key, Modifiers, PointerKind, WheelDelta};
use mln_map::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, MapEvent};
use mln_runtime::{
    AttachOptions, DemandMode, Error, EventMask, EventType, FrameDemand, FrameDisposition, LogicalExtent, Map,
    MapOptions, RenderSession, RenderTarget, Runtime, RuntimeEvent, RuntimeEventKind, RuntimeOptions, Surface,
};
use mln_util::LatLng;
use mln_util::geo::ScreenCoordinate;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_web_host::{pump, shaper};
use parking_lot::Mutex;
use wasm_bindgen::prelude::*;

use crate::gpu_check::GpuCheck;
use crate::js_error;

/// Hands this instance's fetches the responses its Web Worker fetched ahead
/// (web/map-worker.ts): `take(url)` returns a promise of the response
/// fetched for exactly `url`, once, or undefined
/// (`Runtime::set_fetch_preloads`).
#[wasm_bindgen]
pub fn set_preloads(take: js_sys::Function) {
    PRELOADS.with(|preloads| *preloads.borrow_mut() = Some(take));
}

thread_local! {
    /// What `set_preloads` set, for the runtimes made after.
    static PRELOADS: std::cell::RefCell<Option<js_sys::Function>> = const { std::cell::RefCell::new(None) };
}

/// How many phases `WebMap::frame_timings` reports.
const PHASES: usize = 3;

/// What the runtime reports of the map, as its events and queries arrive.
#[derive(Default)]
struct Reports {
    /// The map finished loading its style.
    loaded: bool,
    /// The draw calls of the last frame, and the bytes of the live textures
    /// and of the live buffers after it.
    stats: [f64; 3],
    /// The milliseconds that the frames since the last take spent
    /// rendering, then submitting.
    rendering: [f64; 2],
    /// The style's attributions and name, as the last query read them.
    attributions: Vec<String>,
    style_name: String,
    /// A query of the style's attributions and name is on its way.
    querying: bool,
}

impl Reports {
    fn handle(&mut self, events: &[RuntimeEvent]) {
        for event in events {
            let RuntimeEventKind::Map(event) = &event.kind else { continue };
            match event {
                MapEvent::LoadingFinished => self.loaded = true,
                MapEvent::LoadingFailed(error) => {
                    web_sys::console::error_1(&JsValue::from_str(&format!("map failed to load: {error:?}")));
                }
                MapEvent::RenderFrameFinished(status) => {
                    let stats = &status.rendering_stats;
                    self.stats =
                        [f64::from(stats.num_draw_calls), f64::from(stats.mem_textures), f64::from(stats.mem_buffers)];
                    self.rendering[0] += stats.encoding_time * 1000.0;
                    self.rendering[1] += stats.rendering_time * 1000.0;
                }
                _ => {}
            }
        }
    }
}

/// A continuous map in an `OffscreenCanvas`, in the Web Worker that runs it
/// (web/map-worker.ts).
#[wasm_bindgen]
pub struct WebMap {
    // Dropped first, before the map and its runtime.
    session: RenderSession,
    map: Map,
    _runtime: Runtime,
    reports: Arc<Mutex<Reports>>,
    /// The session has something new to draw since the last demand.
    redraw: Arc<AtomicBool>,
    /// The next demand draws a frame even when nothing changed.
    force: bool,
    /// The map's last command sent before the last demand.
    demanded: u64,
    /// Frames rendered since the last animation frame.
    rendered: u32,
    /// CSS pixels, and device pixels per CSS pixel.
    size: Size,
    pixel_ratio: f64,
    /// The page's pointer, wheel and key input, turned into camera moves.
    gestures: GestureRecognizer,
    /// Milliseconds of the cooperative scheduler's tasks since the last
    /// frame that drew.
    tasks: f64,
    /// What the frames up to the last one that drew spent on each phase
    /// (`frame_timings`).
    phases: [f64; PHASES],
    /// The check of the GPU that `start_gpu_check` started.
    gpu_check: GpuCheck,
}

#[wasm_bindgen]
impl WebMap {
    /// Starts a runtime and its map at `width` x `height` CSS pixels, and
    /// attaches a render session that presents to `canvas` (the page's
    /// canvas, transferred to this worker) with the build's backend: the
    /// browser's WebGPU, or WebGL2 in the `webgl` build. The tiles' workers
    /// run in the Web Workers that `mlnWebConfigureTileWorkers` set up.
    /// `load_shaper()` returns a promise of the shaper module
    /// (`web/pkg/mln_web_shaper.js`), initialized; the map calls it the first
    /// time a style declares font faces. `fallback_font_faces` is the JSON
    /// array of font files that every font stack falls back to, each an
    /// object with a `url` and an optional `unicode-range`, as in a font face
    /// of a style's `font-faces`. With `system_fonts`, the map draws CJK
    /// glyphs in the browser's sans-serif font rather than downloading them,
    /// as GL JS's `localIdeographFontFamily`. `on_frame()` runs in the task
    /// that rendered a frame, while the canvas still has it, and must call
    /// `drain_frames`. `video_host` plays the videos of the style's video
    /// sources, on the page's thread (`mln_video::WebPlayers`).
    ///
    /// Rejects with an error named `NotSupportedError` when the browser's
    /// API can't draw the map.
    #[expect(
        clippy::too_many_arguments,
        reason = "JavaScript passes the canvas, its size and ratio, the map's fonts and the host's callbacks positionally"
    )]
    pub async fn create(
        canvas: web_sys::OffscreenCanvas,
        width: u32,
        height: u32,
        pixel_ratio: f64,
        load_shaper: js_sys::Function,
        fallback_font_faces: Option<String>,
        system_fonts: bool,
        on_frame: js_sys::Function,
        video_host: mln_video::VideoHost,
    ) -> Result<WebMap, JsValue> {
        shaper::set_loader(load_shaper);
        let runtime = Runtime::with_registry(&RuntimeOptions::default(), crate::registry()).map_err(js_error)?;
        sent(runtime.set_fetch_preloads(PRELOADS.with(|preloads| preloads.borrow().clone())));
        let reports = Arc::new(Mutex::new(Reports::default()));
        let handler_reports = Arc::clone(&reports);
        sent(runtime.set_event_handler(move |events| handler_reports.lock().handle(events)));

        let size = Size::new(width.max(1), height.max(1));
        let options = MapOptions {
            map: mln_map::MapOptions { size, pixel_ratio: pixel_ratio as f32, ..Default::default() },
            event_mask: EventMask::NONE
                .with(EventType::MapLoadingFinished)
                .with(EventType::MapLoadingFailed)
                .with(EventType::MapRenderFrameFinished),
            fallback_font_faces,
            system_fonts,
            video_players: Some(Arc::new(mln_video::WebPlayers::new(video_host))),
            ..Default::default()
        };
        let (map, created) = runtime.create_map(options).map_err(js_error)?;

        let redraw = Arc::new(AtomicBool::new(true));
        let redraw_wake = Arc::clone(&redraw);
        let on_frame = LocalObject::new(on_frame);
        let options = AttachOptions {
            frame_wake: Some(Arc::new(move || {
                if let Some(on_frame) = on_frame.get() {
                    let _ = on_frame.call0(&JsValue::NULL);
                }
            })),
            redraw_wake: Some(Arc::new(move || redraw_wake.store(true, Ordering::SeqCst))),
            ..Default::default()
        };
        let surface = Surface::web_canvas(canvas.into(), None).map_err(js_error)?;
        let (session, attached) =
            map.attach(RenderTarget::Surface { size, surface, device: None }, options).map_err(js_error)?;
        created.await.map_err(js_error)?;
        attached.await.map_err(|error| {
            let error = js_sys::Error::new(&format!("the map can't draw: {error}"));
            error.set_name("NotSupportedError");
            JsValue::from(error)
        })?;

        Ok(WebMap {
            session,
            map,
            _runtime: runtime,
            reports,
            redraw,
            force: false,
            demanded: 0,
            rendered: 0,
            size,
            pixel_ratio,
            gestures: GestureRecognizer::default(),
            tasks: 0.0,
            phases: [0.0; PHASES],
            gpu_check: GpuCheck::default(),
        })
    }

    /// Loads a style from a URL.
    pub fn set_style_url(&self, url: &str) {
        sent(self.map.set_style_url(url));
    }

    /// Moves the camera.
    pub fn jump_to(&self, latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) {
        let update = CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: camera_options(latitude, longitude, zoom, bearing, pitch),
            animation: AnimationOptions::default(),
            phase: GesturePhase::None,
        };
        sent(self.map.update_camera(update));
    }

    /// The camera as `[latitude, longitude, zoom, bearing, pitch]`, in
    /// degrees but for the zoom, as of the last command that ran.
    #[must_use]
    pub fn camera(&self) -> Vec<f64> {
        let camera = self.map.snapshot().map(|snapshot| CameraOptions::from(snapshot.view.camera)).unwrap_or_default();
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
        self.gestures.step_to(&mut self.map, &update);
    }

    /// Lets drags and zooms on the globe turn the bearing with the planet,
    /// so that a drag carries on over a pole
    /// (`Map::set_globe_free_bearing_enabled`). Off, as the map starts, they
    /// keep north up.
    pub fn set_globe_free_bearing(&self, free_bearing: bool) {
        sent(self.map.set_globe_free_bearing_enabled(free_bearing));
    }

    /// Jumps to another map's camera, for a comparison that this map
    /// follows: its own motion stops (an ease, momentum, a wheel's eased
    /// zoom).
    pub fn follow(&mut self, latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) {
        sent(self.map.cancel_camera_transitions());
        let update = CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: camera_options(latitude, longitude, zoom, bearing, pitch),
            animation: AnimationOptions::default(),
            phase: GesturePhase::None,
        };
        self.gestures.step_to(&mut self.map, &update);
    }

    /// Reads the style's attributions and name again, for `attributions`
    /// and `style_name` once the read completes, unless a read is on its
    /// way.
    pub fn read_style_info(&self) {
        if std::mem::replace(&mut self.reports.lock().querying, true) {
            return;
        }
        let read = self.map.with_core(|map| {
            let style = map.style();
            let attributions = style
                .get_sources()
                .iter()
                .filter_map(|source| source.get_attribution())
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>();
            (attributions, style.get_name())
        });
        let reports = Arc::clone(&self.reports);
        match read {
            Ok(read) => read.on_complete(move |result| {
                let mut reports = reports.lock();
                reports.querying = false;
                if let Ok((attributions, name)) = result {
                    reports.attributions = attributions;
                    reports.style_name = name;
                }
            }),
            Err(error) => {
                reports.lock().querying = false;
                log_refused(&error);
            }
        }
    }

    /// The attributions of the style's sources that have one, as HTML, as
    /// `read_style_info` last read them. A source that links a TileJSON has
    /// one once the TileJSON loads.
    #[must_use]
    pub fn attributions(&self) -> Vec<String> {
        self.reports.lock().attributions.clone()
    }

    /// The style's `name`, or an empty string, as `read_style_info` last
    /// read it.
    #[must_use]
    pub fn style_name(&self) -> String {
        self.reports.lock().style_name.clone()
    }

    /// Resizes the map and the canvas to `width` x `height` CSS pixels.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        self.size = Size::new(width.max(1), height.max(1));
        self.resize_session()
    }

    /// Draws the map at `pixel_ratio` device pixels per CSS pixel from now
    /// on, as on a display of another density, keeping its tiles, and sizes
    /// the canvas for it.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f64) -> Result<(), JsValue> {
        self.pixel_ratio = pixel_ratio;
        self.resize_session()
    }

    /// One animation frame at `now` (milliseconds, the clock of the input
    /// methods' `time`): steps the gestures' eased zoom and ends those that
    /// time out, then demands a frame of the session if the map has
    /// something new to draw, which it renders in a later task. Returns
    /// whether a frame was rendered since the last animation frame.
    pub fn frame(&mut self, now: f64) -> bool {
        // The scheduler's tasks that the pump ran since the last frame: with
        // no tile workers, the tiles' parsing and layout among them.
        self.tasks += pump::take_scheduler_time().as_secs_f64() * 1000.0;
        self.gestures.frame(&mut self.map, now);
        // A demand renders the map once the commands sent before it ran.
        let sent_sequence = self.map.sent_sequence();
        if self.redraw.swap(false, Ordering::SeqCst) || self.force || sent_sequence != self.demanded {
            let mode = if std::mem::take(&mut self.force) { DemandMode::Force } else { DemandMode::IfChanged };
            self.demanded = sent_sequence;
            sent(self.session.demand_frame(FrameDemand::new(mode, 0)));
        }
        let drew = std::mem::take(&mut self.rendered) > 0;
        if drew {
            let [rendering, submitting] = std::mem::take(&mut self.reports.lock().rendering);
            self.phases = [rendering, submitting, std::mem::take(&mut self.tasks)];
        }
        drew
    }

    /// Takes the session's frame results, which `on_frame` announced;
    /// returns how many of them rendered a frame. Fails once the session
    /// failed, with why.
    pub fn drain_frames(&mut self) -> Result<u32, JsValue> {
        let results = self.session.drain_frame_results().map_err(js_error)?;
        if results.iter().any(|result| result.disposition == FrameDisposition::Failed) {
            let failure = self.session.snapshot().ok().and_then(|snapshot| snapshot.failure);
            return Err(js_error(failure.map_or_else(|| "the render session failed".to_owned(), |e| e.to_string())));
        }
        let rendered = results.iter().filter(|result| result.disposition == FrameDisposition::Rendered).count() as u32;
        self.rendered += rendered;
        Ok(rendered)
    }

    /// The milliseconds of work on this thread since the frame that drew
    /// before the last one that drew, up to its end, by phase: the
    /// renderer's frames (updating the sources and layers, placing symbols,
    /// uploading and encoding), their submission, and the cooperative
    /// scheduler's tasks (with no tile workers, the tiles' parsing and
    /// layout).
    #[must_use]
    pub fn frame_timings(&self) -> Vec<f64> {
        self.phases.to_vec()
    }

    /// The graphics context's counters: the draw calls of the last frame it
    /// drew, and the bytes of its live textures and of its live buffers.
    #[must_use]
    pub fn render_stats(&self) -> Vec<f64> {
        self.reports.lock().stats.to_vec()
    }

    /// A pointer went down on the canvas, at `x`, `y` CSS pixels from its
    /// top-left corner: `kind` is the DOM `pointerType` ("mouse", "touch" or
    /// "pen"), `button` the DOM `button` (0 primary, 1 middle, 2 secondary)
    /// and `modifiers` the bits of [`modifiers`]. `time` is in milliseconds
    /// on the clock `frame` gets. Returns whether the map used it.
    #[expect(clippy::too_many_arguments, reason = "JavaScript passes the DOM pointer event's fields positionally")]
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
        self.gesture(input, time)
    }

    /// A pointer moved (see `pointer_down`).
    pub fn pointer_move(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.gesture(Input::PointerMove { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// A pointer went up (see `pointer_down`).
    pub fn pointer_up(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.gesture(Input::PointerUp { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// The browser took a pointer away (`pointercancel`).
    pub fn pointer_cancel(&mut self, id: u32, time: f64) -> bool {
        self.gesture(Input::PointerCancel { id }, time)
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
        self.gesture(input, time)
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
        self.gesture(Input::Key { key, modifiers: modifiers_from_bits(modifiers) }, time)
    }

    /// The canvas lost focus: every pointer is released.
    pub fn blur(&mut self, time: f64) -> bool {
        self.gesture(Input::Blur, time)
    }

    /// The box a Shift-drag zooms to while it is drawn, as `[x0, y0, x1,
    /// y1]` in CSS pixels, for the page to show; otherwise undefined.
    #[must_use]
    pub fn box_zoom(&self) -> Option<Vec<f64>> {
        self.gestures.box_zoom().map(|[a, b]| vec![a.x, a.y, b.x, b.y])
    }

    /// Whether, after every command sent to the map, the style loaded and
    /// every tile and resource the map needs is loaded and drawn.
    #[must_use]
    pub fn is_fully_loaded(&self) -> bool {
        self.reports.lock().loaded
            && self.map.applied_sequence() >= self.map.sent_sequence()
            && self.map.snapshot().is_ok_and(|snapshot| snapshot.fully_loaded)
    }

    /// Draws a frame with the next animation frame, even if nothing changed,
    /// for the page to read from the canvas in `on_frame`.
    pub fn request_pixels(&mut self) {
        self.force = true;
    }

    /// Starts `mln_gfx`'s conformance case of writes from JS arrays on a
    /// `Gpu` of the map's backend made for the check, whose frame
    /// `take_gpu_check` checks. Only check/check.mjs asks for it.
    pub fn start_gpu_check(&self) {
        self.gpu_check.start();
    }

    /// Whether the GPU has the frame of the check that `start_gpu_check`
    /// started, which passed its checks. Fails when the check couldn't run,
    /// and panics where one of the frame's checks doesn't hold.
    pub fn take_gpu_check(&self) -> Result<bool, JsValue> {
        self.gpu_check.take().map_err(js_error)
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

fn camera_options(latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) -> CameraOptions {
    CameraOptions {
        center: LatLng::new(latitude, longitude).ok(),
        zoom: Some(zoom),
        bearing: Some(bearing),
        pitch: Some(pitch),
        ..Default::default()
    }
}

/// A command's completion, or `None` when the runtime refused it, which it
/// logs: the page doesn't wait for the map's commands.
fn sent<T>(result: Result<T, Error>) -> Option<T> {
    result.map_err(|error| log_refused(&error)).ok()
}

fn log_refused(error: &Error) {
    Log::error(Event::General, format!("the map refused a command: {error}"));
}

impl WebMap {
    /// Feeds one input to the gesture recognizer; returns whether the map
    /// used it.
    fn gesture(&mut self, input: Input, time: f64) -> bool {
        self.gestures.handle(&mut self.map, input, time)
    }

    /// Sizes the session, and the map and canvas with it, to the map's size
    /// and pixel ratio.
    fn resize_session(&self) -> Result<(), JsValue> {
        let extent = LogicalExtent { size: self.size, scale_factor: self.pixel_ratio };
        self.session.resize(extent).map(drop).map_err(js_error)
    }
}
