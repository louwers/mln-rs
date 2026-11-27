//! The map in a Web Worker of its own: a runtime map (`mln_runtime`) whose
//! render session presents to the page's canvas (an `OffscreenCanvas`), with
//! the browser's WebGPU or, in the `webgl` build of the module, its WebGL2.
//! Its tile workers are in other Web Workers, and the page's input moves it
//! through a gesture recognizer (`mln_engine::gestures`).
//!
//! The runtime and the session run on this Web Worker's one thread, in the
//! slices of `mln_web_host`'s pump: a command or a frame demand runs in a
//! later task, never inside the call that sends it. The page's animation
//! frames step the gestures and demand a frame when the map has something
//! new to draw, and the session renders it in a task of its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_engine::camera::{AnimationOptions, BoundOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase};
use mln_engine::geo::ScreenCoordinate;
use mln_engine::gestures::{GestureEvent, GestureRecognizer, Input, Modes, PointerKind};
use mln_engine::map::MapEvent;
use mln_runtime::{
    AttachOptions, DemandMode, Error, ErrorKind, EventMask, EventType, FrameDemand, FrameDisposition, LogicalExtent,
    Map, MapOptions, RenderSession, RenderTarget, Runtime, RuntimeEvent, RuntimeEventKind, RuntimeOptions, Surface,
    ViewportOptions,
};
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_web_host::{pump, shaper};
use parking_lot::Mutex;
use wasm_bindgen::prelude::*;

use crate::gpu_check::GpuCheck;
use crate::js_error;

/// Hands this instance's fetches the responses its Web Worker fetched ahead
/// (web/map-worker.ts): `take(url)` returns a promise of the response
/// fetched for exactly `url`, once, or undefined
/// (`RuntimeOptions::fetch_preloads`).
#[wasm_bindgen]
pub fn set_preloads(take: js_sys::Function) {
    PRELOADS.with(|preloads| *preloads.borrow_mut() = Some(take));
}

thread_local! {
    /// What `set_preloads` set, for the runtimes made after.
    static PRELOADS: std::cell::RefCell<Option<js_sys::Function>> = const { std::cell::RefCell::new(None) };
    /// The `on_frame` of this Web Worker's map, which its session's frame
    /// wake calls on this thread.
    static ON_FRAME: std::cell::RefCell<Option<js_sys::Function>> = const { std::cell::RefCell::new(None) };
}

/// How many phases `WebMap::frame_timings` reports.
const PHASES: usize = 3;

/// What the runtime reports of the map, as its events and queries arrive.
#[derive(Default)]
struct Reports {
    /// The map finished loading its style.
    loaded: bool,
    /// A style document loaded since the last frame, which takes the
    /// page's terrain and projection.
    style_loaded: bool,
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
                MapEvent::StyleLoaded => self.style_loaded = true,
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
    gestures: GestureRecognizer<Map>,
    /// The pointers that went down on the map and haven't lifted.
    pointers: Vec<u32>,
    /// Whether a gesture reached a notch since `take_notch`.
    notched: bool,
    /// Milliseconds of the pump's pool tasks since the last frame that
    /// drew.
    tasks: f64,
    /// What the frames up to the last one that drew spent on each phase
    /// (`frame_timings`).
    phases: [f64; PHASES],
    /// The check of the GPU that `start_gpu_check` started.
    gpu_check: GpuCheck,
    /// The terrain that the page adds to the style (`set_style_url`).
    terrain: Option<Terrain>,
    /// The projection type the page draws with, or `None` for the style's
    /// own (`set_projection`).
    projection: Option<String>,
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
    /// of a style's `font-faces`. With `local_ideographs`, the map draws CJK
    /// glyphs with the browser's fonts rather than downloading them, as GL
    /// JS's `localIdeographFontFamily`. `on_frame()` runs in the task
    /// that rendered a frame, while the canvas still has it, and must call
    /// `drain_frames`.
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
        local_ideographs: bool,
        on_frame: js_sys::Function,
    ) -> Result<WebMap, JsValue> {
        shaper::set_loader(load_shaper);
        let options = RuntimeOptions {
            fetch_preloads: PRELOADS.with(|preloads| preloads.borrow().clone()),
            ..RuntimeOptions::default()
        };
        let runtime = Runtime::with_registry(&options, crate::registry()).map_err(js_error)?;
        let reports = Arc::new(Mutex::new(Reports::default()));
        let handler_reports = Arc::clone(&reports);
        sent(runtime.set_event_handler(move |events| handler_reports.lock().handle(events)));

        let size = Size::new(width.max(1), height.max(1));
        let options = MapOptions {
            map: mln_engine::map::MapOptions { size, pixel_ratio: pixel_ratio as f32, ..Default::default() },
            event_mask: EventMask::NONE
                .with(EventType::MapStyleLoaded)
                .with(EventType::MapLoadingFinished)
                .with(EventType::MapLoadingFailed)
                .with(EventType::MapRenderFrameFinished),
            fallback_font_faces,
            local_ideographs,
            ..Default::default()
        };
        let (map, created) = runtime.create_map(options).map_err(js_error)?;
        // Past the map's default of 60°, to look across terrain.
        sent(map.set_bounds(BoundOptions { max_pitch: Some(MAX_PITCH), ..Default::default() }));

        let redraw = Arc::new(AtomicBool::new(true));
        let redraw_wake = Arc::clone(&redraw);
        ON_FRAME.with(|slot| *slot.borrow_mut() = Some(on_frame));
        let options = AttachOptions {
            frame_wake: Some(Arc::new(|| {
                let on_frame = ON_FRAME.with(|slot| slot.borrow().clone());
                if let Some(on_frame) = on_frame {
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
        attached.await.map_err(|error| not_supported(&error))?;

        Ok(WebMap {
            session,
            map: map.clone(),
            _runtime: runtime,
            reports,
            redraw,
            force: false,
            demanded: 0,
            rendered: 0,
            size,
            pixel_ratio,
            gestures: GestureRecognizer::new(map),
            pointers: Vec::new(),
            notched: false,
            tasks: 0.0,
            phases: [0.0; PHASES],
            gpu_check: GpuCheck::default(),
            terrain: None,
            projection: None,
        })
    }

    /// Loads a style from a URL, and adds `terrain` to it once it loads:
    /// the JSON of web/styles.ts's `Terrain`, which a catalog entry of
    /// examples/styles.json can have. Fails when `terrain` isn't one.
    pub fn set_style_url(&mut self, url: &str, terrain: Option<String>) -> Result<(), JsValue> {
        self.terrain = terrain.map(|json| Terrain::parse(&json)).transpose().map_err(js_error)?;
        sent(self.map.set_style_url(url));
        Ok(())
    }

    /// Loads a style from a JSON string, and adds `terrain` to it once it
    /// loads, as [`set_style_url`](Self::set_style_url) does.
    pub fn set_style_json(&mut self, json: &str, terrain: Option<String>) -> Result<(), JsValue> {
        self.terrain = terrain.map(|json| Terrain::parse(&json)).transpose().map_err(js_error)?;
        sent(self.map.set_style_json(json));
        Ok(())
    }

    /// Draws the map with the style spec's projection type `projection`
    /// (`mercator`, `vertical-perspective` or `globe`) in place of the
    /// style's, now and in each style that loads after; `None` goes back to
    /// the style's own.
    pub fn set_projection(&mut self, projection: Option<String>) {
        self.projection = projection.clone();
        sent(self.map.with_core(move |map| {
            if let Err(error) = set_projection(map, projection.as_deref()) {
                log_refused(&error);
            }
        }));
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
        self.gestures.step_to(&update);
    }

    /// Lets drags and zooms on the globe turn the bearing with the planet,
    /// so that a drag carries on over a pole
    /// (`mln_engine::camera::Viewport::globe_free_bearing`). Off, as the map starts,
    /// they keep north up.
    pub fn set_globe_free_bearing(&self, free_bearing: bool) {
        let options = ViewportOptions { globe_free_bearing: Some(free_bearing), ..Default::default() };
        sent(self.map.set_viewport_options(options));
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
        self.gestures.step_to(&update);
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
                .filter_map(|source| map.registry().source_attribution(source).filter(|a| !a.is_empty()))
                .collect::<Vec<_>>();
            (attributions, style.get_name().to_owned())
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
    /// methods' `time`): ticks the gesture recognizer, then demands a frame
    /// of the session if the map has
    /// something new to draw, which it renders in a later task. Returns
    /// whether a frame was rendered since the last animation frame.
    pub fn frame(&mut self, now: f64) -> bool {
        // The pool tasks that the pump ran since the last frame: with no
        // tile workers, the tiles' parsing and layout among them.
        self.tasks += pump::take_pool_time().as_secs_f64() * 1000.0;
        // The page runs an animation frame always, so it ticks the recognizer
        // on each rather than at its deadlines.
        self.gestures.tick(now);
        self.answer_events();
        if std::mem::take(&mut self.reports.lock().style_loaded) {
            self.customize_style();
        }
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
    /// failed, with why: with an error named `DeviceLostError` when it lost
    /// its device, as a browser's session does when its GPU process restarts.
    pub fn drain_frames(&mut self) -> Result<u32, JsValue> {
        let results = self.session.drain_frame_results().map_err(js_error)?;
        if results.iter().any(|result| result.disposition == FrameDisposition::Failed) {
            let failure = self.session.snapshot().ok().and_then(|snapshot| snapshot.failure);
            return Err(match failure {
                Some(failure) if failure.kind == ErrorKind::TargetLost => named_error("DeviceLostError", &failure),
                failure => js_error(failure.map_or_else(|| "the render session failed".to_owned(), |e| e.to_string())),
            });
        }
        let rendered = results.iter().filter(|result| result.disposition == FrameDisposition::Rendered).count() as u32;
        self.rendered += rendered;
        Ok(rendered)
    }

    /// The milliseconds of work on this thread since the frame that drew
    /// before the last one that drew, up to its end, by phase: the
    /// renderer's frames (updating the sources and layers, placing symbols,
    /// uploading and encoding), their submission, and the pump's pool
    /// tasks (with no tile workers, the tiles' parsing and layout).
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
    /// and `modifiers` the bits of [`SHIFT`] and [`CTRL`]. `time` is in milliseconds
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
        // The secondary button rotates and tilts; the middle one means
        // nothing to the map.
        let modes = match button {
            0 => Modes::default(),
            2 => Modes { rotate: true, ..Modes::default() },
            _ => return false,
        };
        self.gesture(Input::Modes(key_modes(modifiers)), time);
        self.pointers.push(id);
        self.gesture(Input::PointerDown { id, kind, position: ScreenCoordinate::new(x, y), modes }, time)
    }

    /// A pointer moved (see `pointer_down`).
    pub fn pointer_move(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.gesture(Input::PointerMove { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// A pointer went up (see `pointer_down`).
    pub fn pointer_up(&mut self, id: u32, x: f64, y: f64, time: f64) -> bool {
        self.pointers.retain(|&p| p != id);
        self.gesture(Input::PointerUp { id, position: ScreenCoordinate::new(x, y) }, time)
    }

    /// The browser took a pointer away (`pointercancel`).
    pub fn pointer_cancel(&mut self, id: u32, time: f64) -> bool {
        self.pointers.retain(|&p| p != id);
        self.gesture(Input::PointerCancel { id }, time)
    }

    /// A DOM `wheel` event, as the page classifies it (web/main.ts): at
    /// (`x`, `y`), `amount` detents of a mouse wheel when `kind` is
    /// [`WHEEL_NOTCHES`], a third of a level each and eased, or `amount`
    /// pixels of continuous scrolling when it is [`WHEEL_PIXELS`], 1/300
    /// level each; positive scrolls down, zooming out. With Ctrl, which
    /// browsers add to a trackpad pinch, pixels zoom 1/100 level each.
    pub fn wheel(&mut self, x: f64, y: f64, kind: u32, amount: f64, modifiers: u32, time: f64) -> bool {
        debug_assert!(kind == WHEEL_NOTCHES || kind == WHEEL_PIXELS, "wheel kind {kind}");
        let (discrete, zoom) = match kind {
            WHEEL_NOTCHES => (true, -amount / 3.0),
            _ if modifiers & CTRL != 0 => (false, -amount / 100.0),
            _ => (false, -amount / 300.0),
        };
        let position = ScreenCoordinate::new(x, y);
        let pan = ScreenCoordinate::default();
        self.gesture(Input::Delta { position, discrete, pan, zoom, bearing: 0.0, pitch: 0.0 }, time)
    }

    /// A DOM `keydown`, by its `key`, each repeating while held: the arrows
    /// pan 100 px (Shift: turn 15° and tilt 10°), `+`/`=` and `-`/`_` zoom a
    /// level (Shift: two), and Escape drops a drag; the map ignores the
    /// others and returns false. Repeats move half as far as the initial press.
    pub fn key(&mut self, key: &str, modifiers: u32, repeat: bool, time: f64) -> bool {
        self.gesture(Input::Modes(key_modes(modifiers)), time);
        let shift = modifiers & SHIFT != 0;
        let levels = if shift { 2.0 } else { 1.0 };
        let (pan, zoom, bearing, pitch) = match (key, shift) {
            ("Escape", _) => return self.cancel_pointers(time),
            ("+" | "=", _) => ((0.0, 0.0), levels, 0.0, 0.0),
            ("-" | "_", _) => ((0.0, 0.0), -levels, 0.0, 0.0),
            ("ArrowLeft", true) => ((0.0, 0.0), 0.0, -15.0, 0.0),
            ("ArrowRight", true) => ((0.0, 0.0), 0.0, 15.0, 0.0),
            ("ArrowUp", true) => ((0.0, 0.0), 0.0, 0.0, 10.0),
            ("ArrowDown", true) => ((0.0, 0.0), 0.0, 0.0, -10.0),
            ("ArrowLeft", false) => ((100.0, 0.0), 0.0, 0.0, 0.0),
            ("ArrowRight", false) => ((-100.0, 0.0), 0.0, 0.0, 0.0),
            ("ArrowUp", false) => ((0.0, 100.0), 0.0, 0.0, 0.0),
            ("ArrowDown", false) => ((0.0, -100.0), 0.0, 0.0, 0.0),
            _ => return false,
        };
        let position = ScreenCoordinate::new(f64::from(self.size.width) / 2.0, f64::from(self.size.height) / 2.0);
        let factor = if repeat { 0.5 } else { 1.0 };
        let pan = ScreenCoordinate::new(pan.0 * factor, pan.1 * factor);
        self.gesture(
            Input::Delta {
                position,
                discrete: true,
                pan,
                zoom: zoom * factor,
                bearing: bearing * factor,
                pitch: pitch * factor,
            },
            time,
        )
    }

    /// A modifier key went down or up: `modifiers` are the bits held now.
    pub fn modifiers(&mut self, modifiers: u32, time: f64) -> bool {
        self.gesture(Input::Modes(key_modes(modifiers)), time)
    }

    /// Whether a gesture reached a notch since the last call, for the page
    /// to play a haptic.
    pub fn take_notch(&mut self) -> bool {
        std::mem::take(&mut self.notched)
    }

    /// The canvas lost focus: its pointers and keys end here.
    pub fn blur(&mut self, time: f64) -> bool {
        self.gesture(Input::Modes(Modes::default()), time);
        self.cancel_pointers(time)
    }

    /// The box a box-zoom drag zooms to while it is drawn, as `[x0, y0, x1,
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

    /// Starts `mln_engine`'s conformance cases of writes from JS arrays, and
    /// of writes while the instance's memory grows, on a `Gpu` of the map's
    /// backend made for the check, whose frames `take_gpu_check` checks.
    /// Only check/check.mjs asks for it.
    pub fn start_gpu_check(&self) {
        self.gpu_check.start();
    }

    /// Whether the GPU has the frames of the check that `start_gpu_check`
    /// started, which passed their checks. Fails when the check couldn't
    /// run, and panics where one of a frame's checks doesn't hold.
    pub fn take_gpu_check(&self) -> Result<bool, JsValue> {
        self.gpu_check.take().map_err(js_error)
    }
}

/// `WebMap::wheel`'s `kind`, as web/protocol.ts's `WHEEL_PIXELS` and
/// `WHEEL_NOTCHES` send it.
const WHEEL_PIXELS: u32 = 0;
const WHEEL_NOTCHES: u32 = 1;

/// The input methods' `modifiers` bits: Shift 1, Ctrl 2, Alt 4, Meta 8.
const SHIFT: u32 = 1;
const CTRL: u32 = 2;

/// What held keys do: Ctrl rotates and tilts on a drag, and Shift zooms to a
/// box on a drag and out on a double click.
fn key_modes(modifiers: u32) -> Modes {
    let shift = modifiers & SHIFT != 0;
    Modes { rotate: modifiers & CTRL != 0, box_zoom: shift, zoom_out: shift }
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
    log::error!("the map refused a command: {error}");
}

/// An error named `name`, which web/map-worker.ts tells apart from the
/// others by its name.
fn named_error(name: &str, message: &impl std::fmt::Display) -> JsValue {
    let error = js_sys::Error::new(&message.to_string());
    error.set_name(name);
    error.into()
}

/// The browser's API can't draw the map, because of `reason`.
fn not_supported(reason: &impl std::fmt::Display) -> JsValue {
    named_error("NotSupportedError", &format!("the map can't draw: {reason}"))
}

/// How far the page lets the camera tilt, in degrees; compare.ts's GL JS
/// map tilts as far.
const MAX_PITCH: f64 = 85.0;

/// web/styles.ts's `Terrain`.
#[derive(Clone)]
struct Terrain {
    /// The `raster-dem` source's JSON.
    source: String,
    exaggeration: f64,
    hillshade: Option<Hillshade>,
    /// The style's `sky` where it has none, as JSON.
    sky: Option<String>,
}

/// A hillshade layer of a [`Terrain`]'s source.
#[derive(Clone)]
struct Hillshade {
    /// The layer's paint, as JSON.
    paint: String,
    /// The layer that the hillshade goes under, if the style has it; else it
    /// goes on top.
    before: Option<String>,
}

impl Terrain {
    fn parse(json: &str) -> Result<Terrain, String> {
        // The style parser's JSON tree, which the module has already.
        let value: serde_json_borrow::Value<'_> = serde_json::from_str(json).map_err(|e| format!("terrain: {e}"))?;
        let json = |value: &serde_json_borrow::Value<'_>| serde_json::to_string(value).unwrap_or_default();
        let source = value.get("source");
        if !source.is_object() {
            return Err("terrain: no source".to_owned());
        }
        let hillshade = value.get("hillshade");
        let hillshade = (!hillshade.is_null()).then(|| {
            let paint = hillshade.get("paint");
            Hillshade {
                paint: if paint.is_object() { json(paint) } else { "{}".to_owned() },
                before: hillshade.get("before").as_str().map(str::to_owned),
            }
        });
        let sky = value.get("sky");
        Ok(Terrain {
            source: json(source),
            exaggeration: value.get("exaggeration").as_f64().ok_or("terrain: no exaggeration")?,
            hillshade,
            sky: sky.is_object().then(|| json(sky)),
        })
    }
}

/// The ID of the source that `add_terrain` adds.
const TERRAIN_SOURCE: &str = "terrain";

/// Adds `terrain` to the style that `map` loaded, unless it has the source
/// already: the source, the style's `terrain` on it, the hillshade, if any,
/// and the sky, if any and the style has none. Fails while no style is loaded.
fn add_terrain(map: &mut mln_engine::map::Map, terrain: &Terrain) -> Result<(), Error> {
    if map.source(TERRAIN_SOURCE).is_some() {
        return Ok(());
    }
    map.add_source_json(TERRAIN_SOURCE, &terrain.source)?;
    map.edit_loaded_style(|style| {
        if let Some(hillshade) = &terrain.hillshade {
            let layer = format!(
                r#"{{"id":"terrain-hillshade","type":"hillshade","source":"{TERRAIN_SOURCE}","paint":{}}}"#,
                hillshade.paint
            );
            let before = hillshade.before.as_deref().filter(|id| style.get_layer(id).is_some());
            style.add_layer_json(&layer, before)?;
        }
        let root = format!(r#"{{"source":"{TERRAIN_SOURCE}","exaggeration":{}}}"#, terrain.exaggeration);
        style.set_root_property_json("terrain", &root)?;
        match &terrain.sky {
            Some(sky) if style.get_sky().is_none() => style.set_root_property_json("sky", sky),
            _ => Ok(()),
        }
    })
}

/// Sets the style's `projection` to the type `projection`, or for `None`
/// back to the style document's own. Does nothing while no style is loaded:
/// the map sets it once one loads. Fails for a type other than `mercator`,
/// `vertical-perspective` and `globe`.
fn set_projection(map: &mut mln_engine::map::Map, projection: Option<&str>) -> Result<(), Error> {
    if !map.style_document_loaded() {
        return Ok(());
    }
    let json = match projection {
        Some(kind @ ("mercator" | "vertical-perspective" | "globe")) => format!(r#"{{"type":"{kind}"}}"#),
        Some(kind) => return Err(Error::invalid_argument(format!("no projection type {kind:?}"))),
        // The style parser's JSON tree, which the module has already.
        None => serde_json::from_str::<serde_json_borrow::Value<'_>>(map.style().get_json())
            .ok()
            .and_then(|document| serde_json::to_string(document.get("projection")).ok())
            .unwrap_or_else(|| "null".to_owned()),
    };
    map.edit_loaded_style(|style| style.set_root_property_json("projection", &json))
}

impl WebMap {
    /// Adds the page's terrain to the style that loaded, and draws it with
    /// the page's projection. A command that runs once another style has
    /// started loading changes nothing.
    fn customize_style(&self) {
        let (terrain, projection) = (self.terrain.clone(), self.projection.clone());
        if terrain.is_none() && projection.is_none() {
            return;
        }
        sent(self.map.with_core(move |map| {
            let edited = terrain.as_ref().map_or(Ok(()), |terrain| add_terrain(map, terrain));
            let edited = edited.and_then(|()| match &projection {
                Some(projection) => set_projection(map, Some(projection)),
                None => Ok(()),
            });
            if let Err(error) = edited {
                log_refused(&error);
            }
        }));
    }

    /// Feeds one input to the gesture recognizer, and answers its events.
    /// Returns whether the map used the input.
    fn gesture(&mut self, input: Input, time: f64) -> bool {
        let used = self.gestures.handle(input, time);
        self.answer_events();
        used
    }

    /// Answers the recognizer's events: a notch reaches the page. The page
    /// has no use for taps.
    fn answer_events(&mut self) {
        for event in self.gestures.take_events() {
            if let GestureEvent::Notch { .. } = event {
                self.notched = true;
            }
        }
    }

    /// Cancels every pointer down on the map; returns whether there was one.
    fn cancel_pointers(&mut self, time: f64) -> bool {
        let pointers = std::mem::take(&mut self.pointers);
        for &id in &pointers {
            self.gesture(Input::PointerCancel { id }, time);
        }
        !pointers.is_empty()
    }

    /// Sizes the session, and the map and canvas with it, to the map's size
    /// and pixel ratio.
    fn resize_session(&self) -> Result<(), JsValue> {
        let extent = LogicalExtent { size: self.size, scale_factor: self.pixel_ratio };
        self.session.resize(extent).map(drop).map_err(js_error)
    }
}
