// The render thread: the RunLoop, the map, its renderer and the window's
// surface, after platform/glfw/glfw_view.cpp and main.cpp.
//
// The thread sleeps in the RunLoop until it has work: a message from the
// window (host.rs), a task (a tile or style response) or a timer. A frame
// waits for the surface's next texture first, then takes the messages that
// came meanwhile, so that it draws the latest input. Frames come at most once
// per refresh of the display the window opened on, or as fast as they can in
// benchmark mode.
//
// Mouse, touch, wheel, trackpad and arrow-key input goes to a
// `GestureRecognizer` (mln_gestures), which replaces the C++ view's
// hand-rolled pan, zoom, rotate and pitch handlers and continues releases
// with the map's momentum.
//
// Key bindings: those that map onto ported APIs (see `on_key`); the others
// (annotations, snapshots, queries, custom sources, extrusion/route/feature
// state demos, render-test writer, free camera demo, location indicator,
// rendering stats) log that they are not ported. TODO(port) each.
//
// For checking frames without screenshots: `--readback <file.png>` writes the
// presented frame once the map is idle (or at exit), `--quit-after <seconds>`
// closes the window; the style report prints once which layers and sources
// the renderer skips.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use mln_actor::{RunLoop, Timer};
use mln_gestures::{Button, GestureRecognizer, Input, Modifiers, PinchPhase, PointerKind, WheelDelta};
use mln_gfx::{BackendScope, RendererBackend, ScopeType};
use mln_map::{
    AnimationOptions, BoundOptions, CameraDelta, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, Map,
    MapDebugOptions, MapObserver, MapOptions, NorthOrientation, RenderFrameStatus, TileLodMode,
};
use mln_renderer::Renderer;
use mln_storage::{ClientOptions, FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_util::chrono::{Clock, Duration};
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::tile_server_options::{DefaultStyle, TileServerOptions};
use mln_webgpu::SurfaceBackend;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::KeyCode;

use crate::Args;
use crate::frontend::Frontend;
use crate::host::{Inbox, Message, UserEvent};
use crate::settings::Settings;

/// What the render thread starts with.
pub(crate) struct Launch {
    pub args: Args,
    pub settings: Settings,
    /// The window's scale factor when it opened: the map's pixel ratio.
    pub pixel_ratio: f64,
    /// The window's size in physical pixels.
    pub size: PhysicalSize<u32>,
    /// The least time between frames, outside benchmark mode.
    pub frame_interval: std::time::Duration,
    /// The epoch of the gesture recognizer's clock.
    pub start: Instant,
    pub inbox: Arc<Inbox>,
    pub proxy: EventLoopProxy<UserEvent>,
}

/// What the frontend and the map observer reach while the view is busy.
pub struct ViewShared {
    dirty: Cell<bool>,
    style_loaded: Cell<bool>,
    idle: Cell<bool>,
}

impl ViewShared {
    /// `invalidate()`: the map has a new frame to draw.
    pub fn invalidate(&self) {
        self.dirty.set(true);
    }
}

/// The view's `MapObserver`.
struct Observer {
    shared: Rc<ViewShared>,
}

impl MapObserver for Observer {
    fn on_did_finish_loading_style(&self) {
        // C++: if (show3DExtrusions) toggle3DExtrusions(show3DExtrusions);
        // TODO(port): 3D extrusions. The view reports the style once.
        self.shared.style_loaded.set(true);
    }

    fn on_will_start_rendering_frame(&self) {
        // TODO(port): updatePuckState() (location indicator).
    }

    fn on_did_finish_rendering_frame(&self, _status: &RenderFrameStatus) {
        // TODO(port): rendered feature logging (status.renderingStats).
    }

    fn on_did_become_idle(&self) {
        self.shared.idle.set(true);
    }
}

const HELP: &str = "
================================================================================

- Press `X` to reset the transform
- Press `N` to reset north
- Press `O` to toggle online connectivity
- Press `S` to cycle through bundled styles
- Press `Z` to cycle through north orientations
- Press `A` to cycle through Mapbox offices in the world + dateline monument
- Press `U` to toggle pitch bounds
- Press `P` to pause tile requests
- Press `+`/`-` to zoom in/out
- Press the arrow keys to pan, `Shift` + left/right to rotate, `Shift` + up/down to tilt

- Mouse drag to pan; release while moving to glide
- Right mouse drag, or `Control` + mouse drag, to rotate and tilt
- Hold `Shift` + mouse drag to zoom to a box (the box isn't drawn)
- Scroll to zoom (a wheel notch eases a third of a level); double click to zoom in, `Shift` + double click to zoom out
- Pinch and twist a trackpad (macOS) or a touchscreen to zoom and rotate

- Press `Tab` to cycle through the map debug options
- Press `F6` to cycle through Tile LOD modes
- Press `F7` to lower the zoom level without changing the camera
- Press `F8` to higher the zoom level without changing the camera
- Press `Esc` to quit (or to cancel a box zoom)

(Not ported yet: annotations 1-0/K/L/W/M/C/Q, B, D, E, F, F1, G, H, I, J, R, T, Y.)

================================================================================
";

/// The OpenFreeMap Liberty style, which opens without `--style` or
/// MLN_STYLE_URL.
const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

/// Opens the map in the window and runs it until the window closes.
pub(crate) fn run(launch: Launch) {
    let Launch { args, mut settings, pixel_ratio, size, frame_interval, start, inbox, proxy } = launch;
    let run_loop = RunLoop::new();
    inbox.set_waker(run_loop.handle());

    let framebuffer_size = Size::new(size.width, size.height);
    let surface_proxy = proxy.clone();
    let make_surface = move |instance: &wgpu::Instance| {
        let (reply, surface) = mpsc::channel();
        surface_proxy
            .send_event(UserEvent::MakeSurface(instance.clone(), reply))
            .map_err(|_| "The window closed before it had a surface".to_owned())?;
        surface.recv().map_err(|_| "The window closed before it had a surface".to_owned())?
    };
    // Vsync unless benchmarking.
    let backend = match SurfaceBackend::new(make_surface, framebuffer_size, !args.benchmark) {
        Ok(backend) => Rc::new(backend),
        Err(e) => {
            Log::error(Event::GraphicsBackend, &e);
            return;
        }
    };
    let info = backend.get_adapter_info();
    Log::info(
        Event::Render,
        format!(
            "Rendering with {} ({:?}, driver {} {}), surface format {:?}",
            info.name,
            info.backend,
            info.driver,
            info.driver_info,
            backend.get_swap_chain_format()
        ),
    );
    print!("{HELP}");

    if args.benchmark {
        Log::info(Event::General, "BENCHMARK MODE: Some optimizations are disabled.");
    }

    // Set access token if present
    let apikey = args.apikey.clone().or_else(|| std::env::var("MLN_API_KEY").ok()).unwrap_or_default();
    let cache_db = args.cache.clone().unwrap_or_else(|| "/tmp/mbgl-cache.db".to_owned());
    let map_tiler_configuration = TileServerOptions::map_tiler_configuration();
    let resource_options = ResourceOptions::default()
        .with_cache_path(cache_db)
        .with_api_key(apikey)
        .with_tile_server_options(map_tiler_configuration.clone());
    let client_options = ClientOptions::default();

    let manager = FileSourceManager::get();
    let online_file_source = manager.get_file_source(FileSourceType::Network, &resource_options, &client_options);
    if !settings.online {
        match &online_file_source {
            Some(online) => {
                online.set_property("online-status", &feature::Value::Bool(false));
                Log::warning(Event::Setup, "Application is offline. Press `O` to toggle online status.");
            }
            None => Log::warning(
                Event::Setup,
                "Network resource provider is not available, only local requests are supported.",
            ),
        }
    }
    // Resource loader controls top-level request processing and can resume /
    // pause all managed sources simultaneously.
    let resource_loader = manager.get_file_source(FileSourceType::ResourceLoader, &resource_options, &client_options);

    let shared =
        Rc::new(ViewShared { dirty: Cell::new(false), style_loaded: Cell::new(false), idle: Cell::new(false) });
    let renderer_backend: Rc<dyn RendererBackend> = backend.clone();
    let renderer = Renderer::new(renderer_backend.clone(), pixel_ratio as f32, None);
    let frontend = Frontend::new(renderer, renderer_backend, shared.clone());
    let map_size = logical_size(size, pixel_ratio);

    // TODO(port): ActionJournalOptions, withRenderedFeatureInfo(showFeatures).
    let mut map = match Map::with_resource_options(
        &frontend,
        Rc::new(Observer { shared: shared.clone() }),
        MapOptions::new().with_size(map_size).with_pixel_ratio(pixel_ratio as f32),
        &resource_options,
        &client_options,
    ) {
        Ok(map) => map,
        Err(e) => {
            Log::error(Event::Setup, e);
            return;
        }
    };

    if let Err(e) = map.set_bounds(&BoundOptions::default().with_max_pitch(settings.max_pitch)) {
        Log::warning(Event::Setup, e);
    }
    map.jump_to(
        &CameraOptions::default()
            .with_center(LatLng::new(settings.latitude, settings.longitude).ok())
            .with_center_altitude(Some(settings.altitude))
            .with_zoom(Some(settings.zoom))
            .with_bearing(Some(settings.bearing))
            .with_pitch(Some(settings.pitch))
            .with_roll(Some(settings.roll))
            .with_fov(Some(settings.fov)),
    );
    map.set_debug(MapDebugOptions(settings.debug));

    let mut view = View {
        run_loop,
        wake: Timer::new(),
        inbox,
        proxy,
        start,
        frame_interval,
        next_frame_at: Instant::now(),
        backend,
        frontend,
        map,
        shared,
        gestures: GestureRecognizer::default(),
        gestures_animating: false,
        benchmark: args.benchmark,
        pixel_ratio,
        size: map_size,
        quit: false,
        frames: 0,
        frame_time: 0.0,
        last_reported: Instant::now(),
        online: settings.online,
        online_file_source,
        resource_loader,
        paused: false,
        ordered_styles: map_tiler_configuration.default_styles(),
        style_index: 0,
        next_place: 0,
        tile_lod_defaults: None,
        tile_lod_profile: 0,
        gesture_test: args.gesture_test.then_some(GestureTest { step: 0, next_at: None }),
        readback_path: args.readback.as_ref().map(PathBuf::from),
        readback_requested: false,
        quit_at: args.quit_after.map(|s| Instant::now() + std::time::Duration::from_secs_f64(s)),
        style_reported: false,
    };

    // Load style
    let style = match &args.style {
        Some(style) if !style.contains("://") => format!("file://{style}"),
        Some(style) => style.clone(),
        None => match std::env::var("MLN_STYLE_URL") {
            Ok(url) => {
                view.set_window_title(&url);
                url
            }
            Err(_) => {
                view.set_window_title("OpenFreeMap Liberty");
                LIBERTY.to_owned()
            }
        },
    };
    view.map.get_style_mut().load_url(&style);

    view.run();

    // Save settings
    let camera = view.map.get_camera_options(None);
    if let Some(center) = camera.center {
        settings.latitude = center.latitude();
        settings.longitude = center.longitude();
    }
    settings.altitude = camera.center_altitude.unwrap_or(settings.altitude);
    settings.zoom = camera.zoom.unwrap_or(settings.zoom);
    settings.bearing = camera.bearing.unwrap_or(settings.bearing);
    settings.pitch = camera.pitch.unwrap_or(settings.pitch);
    settings.roll = camera.roll.unwrap_or(settings.roll);
    settings.fov = camera.fov.unwrap_or(settings.fov);
    // TODO(port): settings.save().
    Log::info(
        Event::General,
        format!(
            "Exit location: --lat=\"{:.6}\" --lon=\"{:.6}\" --alt=\"{:.6}\" --zoom=\"{:.6}\" --bearing=\"{:.6}\" --roll=\"{:.6}\" --fov=\"{:.6}\" --maxPitch=\"{:.6}\"",
            settings.latitude,
            settings.longitude,
            settings.altitude,
            settings.zoom,
            settings.bearing,
            settings.roll,
            settings.fov,
            settings.max_pitch
        ),
    );
    // Drop order as in C++: the map resets the renderer, then the frontend,
    // then the backend's surface, while the window is still open.
    let View { map, frontend, backend, .. } = view;
    drop(map);
    drop(frontend);
    drop(backend);
}

/// A physical size in the map's logical pixels.
fn logical_size(size: PhysicalSize<u32>, pixel_ratio: f64) -> Size {
    let size: LogicalSize<u32> = size.to_logical(pixel_ratio);
    Size::new(size.width, size.height)
}

/// Logs a camera change's error (a NaN pan from the input).
fn log_error(result: Result<(), String>) {
    if let Err(e) = result {
        Log::error(Event::General, &e);
    }
}

fn animation_ms(ms: i64) -> AnimationOptions {
    AnimationOptions::new(Duration::from_millis(ms))
}

/// `+` and `-`: the C++ binding's 500 ms scale around the cursor.
fn key_zoom(scale: f64, at: ScreenCoordinate) -> CameraDelta {
    CameraDelta { scale: Some(scale), anchor: Some(at), animation: animation_ms(500), ..CameraDelta::default() }
}

struct View {
    run_loop: RunLoop,
    /// Wakes the RunLoop when the next frame or a deadline is due.
    wake: Timer,
    inbox: Arc<Inbox>,
    proxy: EventLoopProxy<UserEvent>,
    start: Instant,
    frame_interval: std::time::Duration,
    next_frame_at: Instant,

    backend: Rc<SurfaceBackend>,
    frontend: Frontend,
    map: Map,
    shared: Rc<ViewShared>,
    gestures: GestureRecognizer,
    /// The recognizer eases a scroll, and wants frames.
    gestures_animating: bool,

    benchmark: bool,
    pixel_ratio: f64,
    /// The map's size, in logical pixels.
    size: Size,
    quit: bool,

    // Frame timer
    frames: i32,
    frame_time: f32,
    last_reported: Instant,

    online: bool,
    online_file_source: Option<Arc<dyn FileSource>>,
    resource_loader: Option<Arc<dyn FileSource>>,
    paused: bool,
    ordered_styles: Vec<DefaultStyle>,
    style_index: usize,

    gesture_test: Option<GestureTest>,
    readback_path: Option<PathBuf>,
    readback_requested: bool,
    quit_at: Option<Instant>,
    style_reported: bool,

    // C++ function-local statics.
    next_place: usize,
    tile_lod_defaults: Option<(f64, f64, f64, TileLodMode)>,
    tile_lod_profile: u8,
}

impl View {
    /// Milliseconds on the gesture recognizer's clock.
    fn ms(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.start).as_secs_f64() * 1000.0
    }

    /// `setWindowTitle(const std::string&)`
    fn set_window_title(&self, title: &str) {
        let _ = self.proxy.send_event(UserEvent::SetTitle(format!("MapLibre Native: {title}")));
    }

    /// Frames and work until the window closes. A readback that never
    /// happened (the map never became idle) is taken from one last frame.
    fn run(&mut self) {
        while !self.quit {
            self.run_loop.run_once();
            if self.quit {
                break;
            }
            let now = Instant::now();
            if !self.frame_due(now) {
                self.wait_until(self.next_deadline());
                continue;
            }
            if !self.benchmark && now < self.next_frame_at {
                self.wait_until(Some(self.next_frame_at));
                continue;
            }
            self.next_frame_at = now + self.frame_interval;
            self.frame();
        }
        if self.readback_path.is_some() {
            self.backend.request_readback();
            self.shared.dirty.set(true);
            self.render();
            self.save_readback();
        }
    }

    /// Whether a frame has something to do.
    fn frame_due(&self, now: Instant) -> bool {
        self.shared.dirty.get()
            || self.gestures_animating
            || self.benchmark
            || !self.inbox.is_empty()
            || self.next_deadline().is_some_and(|at| at <= now)
            || (self.readback_path.is_some() && (self.readback_requested || self.shared.idle.get()))
    }

    /// When a frame is due without input: the gesture test's next step, or
    /// the quit timer.
    fn next_deadline(&self) -> Option<Instant> {
        let test = self.gesture_test.as_ref().and_then(|t| t.next_at.or(self.style_reported.then_some(self.start)));
        match (test, self.quit_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Sleeps in the RunLoop until it has work, or until `deadline`.
    fn wait_until(&self, deadline: Option<Instant>) {
        match deadline {
            Some(at) => {
                let nanos = at.saturating_duration_since(Instant::now()).as_nanos();
                self.wake.start(
                    Duration::from_nanos(i64::try_from(nanos).unwrap_or(i64::MAX)),
                    Duration::zero(),
                    || {},
                );
            }
            None => self.wake.stop(),
        }
        self.run_loop.wait_for_work();
    }

    /// One frame: the surface's next texture, the messages that came while
    /// waiting for it, the recognizer's eases, and the map's frame.
    fn frame(&mut self) {
        // Wait for the display first, so that the frame takes the input that
        // came meanwhile.
        let drawable = (self.shared.dirty.get() || !self.inbox.is_empty()) && self.backend.acquire_next_texture();

        // The map samples its animations at this frame's time.
        self.map.set_frame_time(Some(Clock::now()));
        for message in self.inbox.take() {
            self.handle(message);
        }
        let now = self.ms(Instant::now());
        self.gestures_animating = self.gestures.frame(&mut self.map, now);

        // A perf probe: `MLN_PAN_PX=<px>` pans the map by that many pixels
        // every frame, reversing every 2 s (so it stays over the same area),
        // so the "Frame time" log measures continuous camera movement.
        if let Some(dx) = std::env::var("MLN_PAN_PX").ok().and_then(|v| v.parse::<f64>().ok()) {
            let dx = if (now / 2000.0) as i64 % 2 == 0 { dx } else { -dx };
            log_error(self.map.move_by(&ScreenCoordinate::new(dx, 0.0), &AnimationOptions::default()));
            self.shared.invalidate();
        }

        // No texture right now (e.g. a timeout while the compositor is busy,
        // or a hidden window): try again on the next frame.
        if drawable || (self.shared.dirty.get() && self.backend.acquire_next_texture()) {
            self.render();
        }
        self.after_frame();
    }

    /// Applies one message from the window.
    fn handle(&mut self, message: Message) {
        match message {
            Message::Input { input, at } => self.gesture(input, at),
            Message::Key { code, pressed, modifiers, cursor, at } => self.on_key(code, pressed, modifiers, cursor, at),
            Message::Resized(size) => self.on_resize(size),
            Message::Focused { focused, at } => {
                if !focused {
                    // A drag the window can't see the end of ends here.
                    self.gesture(Input::Blur, at);
                    // Focus lost.
                    self.frontend.with_renderer(|renderer| renderer.reduce_memory_use());
                }
            }
            Message::Close => self.quit = true,
        }
    }

    /// Feeds one input to the gesture recognizer.
    fn gesture(&mut self, input: Input, at: Instant) {
        let at = self.ms(at);
        self.gestures.handle(&mut self.map, input, at);
        // The app has no use for clicks.
        self.gestures.take_events();
    }

    /// One of the C++ bindings' eased steps, through the recognizer, so the
    /// camera reports it as a gesture and it doesn't end a drag or lose a
    /// running notch ease's zoom.
    fn key_step(&mut self, step: CameraDelta) {
        log_error(self.gestures.step(&mut self.map, step).map_err(|e| format!("{e:?}")));
    }

    fn on_resize(&mut self, size: PhysicalSize<u32>) {
        self.backend.set_framebuffer_size(Size::new(size.width, size.height));
        self.size = logical_size(size, self.pixel_ratio);
        // C++ throws for an empty size (a minimized window).
        if let Err(e) = self.map.set_size(self.size) {
            Log::warning(Event::General, e);
        }
        self.shared.invalidate();
    }

    /// `onKey`. The arrow keys go to the gesture recognizer, which pans,
    /// rotates and tilts with them (C++ binds none of them), and repeat while
    /// held. The other bindings act on release, as in C++.
    fn on_key(&mut self, code: KeyCode, pressed: bool, mods: Modifiers, cursor: ScreenCoordinate, at: Instant) {
        let arrow = match code {
            KeyCode::ArrowLeft => Some(mln_gestures::Key::Left),
            KeyCode::ArrowRight => Some(mln_gestures::Key::Right),
            KeyCode::ArrowUp => Some(mln_gestures::Key::Up),
            KeyCode::ArrowDown => Some(mln_gestures::Key::Down),
            _ => None,
        };
        if let Some(arrow) = arrow {
            if pressed {
                self.gesture(Input::Key { key: arrow, modifiers: mods }, at);
            }
            return;
        }
        if pressed {
            // TODO(port): W, 1-0, M (annotations) on press/repeat.
            return;
        }
        let no_mods = mods == Modifiers::default();
        match code {
            // Escape cancels a box zoom being drawn, else quits.
            KeyCode::Escape => {
                if self.gestures.box_zoom().is_some() {
                    self.gesture(Input::Key { key: mln_gestures::Key::Escape, modifiers: mods }, at);
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Tab => self.cycle_debug_options(),
            // The C++ bindings' steps, through the recognizer, so the camera
            // reports them as the user's, as the recognizer's keys are.
            KeyCode::NumpadAdd | KeyCode::Equal => self.key_step(key_zoom(1.1, cursor)),
            KeyCode::NumpadSubtract | KeyCode::Minus => self.key_step(key_zoom(1.0 / 1.1, cursor)),
            KeyCode::KeyX if no_mods => self.map.jump_to(
                &CameraOptions::default()
                    .with_center(Some(LatLng::default()))
                    .with_zoom(Some(0.0))
                    .with_bearing(Some(0.0))
                    .with_pitch(Some(0.0)),
            ),
            KeyCode::KeyO => self.toggle_online(),
            KeyCode::KeyS => self.change_style(),
            KeyCode::KeyN if no_mods => {
                let north = CameraUpdate {
                    mode: CameraUpdateMode::Ease,
                    camera: CameraOptions::default().with_bearing(Some(0.0)),
                    animation: animation_ms(500),
                    phase: GesturePhase::None,
                };
                self.gestures.step_to(&mut self.map, north);
            }
            KeyCode::KeyZ => self.next_orientation(),
            KeyCode::KeyP => self.pause_resume(),
            KeyCode::KeyI => {
                // TODO(port): DatabaseFileSource::resetDatabase (the write path).
                Log::warning(Event::Database, "Resetting the cache database is not ported yet");
            }
            KeyCode::KeyA => {
                // XXX Fix precision loss in flyTo:
                // https://github.com/mapbox/mapbox-gl-native/issues/4298
                let places = [
                    (-16.796665, -179.999983), // Dateline monument
                    (12.9810542, 77.6345551),  // Mapbox Bengaluru, India
                    (-13.15607, -74.21773),    // Mapbox Peru
                    (37.77572, -122.4158818),  // Mapbox SF, USA
                    (38.91318, -77.03255),     // Mapbox DC, USA
                ];
                let (lat, lng) = places[self.next_place];
                self.next_place = (self.next_place + 1) % places.len();
                let camera = CameraOptions::default()
                    .with_center(LatLng::new(lat, lng).ok())
                    .with_zoom(Some(20.0))
                    .with_pitch(Some(30.0));
                self.map.fly_to(&camera, &AnimationOptions::new(Duration::from_secs(10)));
            }
            KeyCode::KeyD => {
                // TODO(port): the bounds annotation.
                Log::warning(Event::General, "Camera bounds cycling is not ported yet");
            }
            KeyCode::KeyU => {
                let bounds = self.map.get_bounds();
                if bounds.min_pitch == Some(mln_math::rad2deg(mln_util::constants::PITCH_MIN))
                    && bounds.max_pitch == Some(mln_math::rad2deg(mln_util::constants::PITCH_MAX))
                {
                    Log::info(Event::General, "Limiting pitch bounds to [30, 40] degrees");
                    let _ = self.map.set_bounds(&BoundOptions::default().with_min_pitch(30.0).with_max_pitch(40.0));
                } else {
                    Log::info(Event::General, "Resetting pitch bounds to [0, 60] degrees");
                    let _ = self.map.set_bounds(&BoundOptions::default().with_min_pitch(0.0).with_max_pitch(60.0));
                }
            }
            KeyCode::F6 => self.cycle_tile_lod_profile(),
            KeyCode::F7 => self.tile_lod_zoom_shift(false),
            KeyCode::F8 => self.tile_lod_zoom_shift(true),
            KeyCode::KeyQ
            | KeyCode::KeyC
            | KeyCode::KeyB
            | KeyCode::KeyK
            | KeyCode::KeyL
            | KeyCode::KeyR
            | KeyCode::KeyE
            | KeyCode::KeyT
            | KeyCode::KeyF
            | KeyCode::F1
            | KeyCode::KeyH
            | KeyCode::KeyJ
            | KeyCode::KeyG
            | KeyCode::KeyY => {
                Log::warning(Event::General, format!("Key {code:?}: not ported yet"));
            }
            _ => {}
        }
    }

    /// `O`: main.cpp's online status callback.
    fn toggle_online(&mut self) {
        let Some(online_file_source) = &self.online_file_source else {
            Log::warning(Event::Setup, "Cannot change online status. Network resource provider is not available.");
            return;
        };
        self.online = !self.online;
        online_file_source.set_property("online-status", &feature::Value::Bool(self.online));
        Log::info(
            Event::Setup,
            format!(
                "Application is {}. Press `O` to toggle online status.",
                if self.online { "online" } else { "offline" }
            ),
        );
    }

    /// `S`: main.cpp's change style callback.
    fn change_style(&mut self) {
        if self.ordered_styles.is_empty() {
            return;
        }
        self.style_index = (self.style_index + 1) % self.ordered_styles.len();
        let new_style = self.ordered_styles[self.style_index].clone();
        self.map.get_style_mut().load_url(new_style.get_url());
        self.set_window_title(new_style.get_name());
        Log::info(Event::Setup, format!("Changed style to: {}", new_style.get_name()));
    }

    /// `P`: main.cpp's pause/resume callback.
    fn pause_resume(&mut self) {
        if let Some(resource_loader) = &self.resource_loader {
            if self.paused {
                resource_loader.resume();
            } else {
                resource_loader.pause();
            }
        }
        self.paused = !self.paused;
    }

    /// `render()`
    fn render(&mut self) {
        if !self.shared.dirty.get() {
            return;
        }
        self.shared.dirty.set(false);
        let started = Instant::now();

        // TODO(port): animateRouteCallback, updateAnimatedAnnotations.
        {
            let backend: Rc<dyn RendererBackend> = self.backend.clone();
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            self.frontend.render();
        }
        // TODO(port): updateFreeCameraDemo.

        self.report((1000.0 * started.elapsed().as_secs_f64()) as f32);
        if self.benchmark {
            self.shared.invalidate();
        }
    }

    /// `report(float duration)`
    fn report(&mut self, duration: f32) {
        self.frames += 1;
        self.frame_time += duration;
        if self.last_reported.elapsed().as_secs_f64() >= 1.0 {
            self.frame_time /= self.frames as f32;
            Log::info(
                Event::Render,
                format!("Frame time: {:.2}ms ({:.2}fps)", self.frame_time, 1000.0 / self.frame_time),
            );
            self.frames = 0;
            self.frame_time = 0.0;
            self.last_reported = Instant::now();
        }
    }

    /// The checks after each frame: the style report, the gesture test, the
    /// readback and the quit timer.
    fn after_frame(&mut self) {
        if self.style_reported {
            self.run_gesture_test();
        }

        if self.shared.style_loaded.get() && !self.style_reported {
            self.style_reported = true;
            report_style(&self.map);
        }

        if self.readback_path.is_some() {
            if self.readback_requested {
                self.save_readback();
            } else if self.shared.idle.get() && self.style_reported {
                self.backend.request_readback();
                self.readback_requested = true;
                self.shared.invalidate();
            }
        }

        if let Some(quit_at) = self.quit_at
            && Instant::now() >= quit_at
        {
            Log::info(Event::General, "--quit-after: closing the window");
            self.quit = true;
        }
    }

    fn with_map(&mut self, f: impl FnOnce(&mut Map)) {
        f(&mut self.map);
    }

    /// `cycleDebugOptions()`
    fn cycle_debug_options(&mut self) {
        self.with_map(|map| {
            let mut debug = map.get_debug();
            if debug.contains(MapDebugOptions::OVERDRAW) {
                debug = MapDebugOptions::NO_DEBUG;
            } else if debug.contains(MapDebugOptions::COLLISION) {
                debug = MapDebugOptions::OVERDRAW;
            } else if debug.contains(MapDebugOptions::TIMESTAMPS) {
                debug |= MapDebugOptions::COLLISION;
            } else if debug.contains(MapDebugOptions::PARSE_STATUS) {
                debug |= MapDebugOptions::TIMESTAMPS;
            } else if debug.contains(MapDebugOptions::TILE_BORDERS) {
                debug |= MapDebugOptions::PARSE_STATUS;
            } else {
                debug = MapDebugOptions::TILE_BORDERS;
            }
            map.set_debug(debug);
        });
    }

    /// `nextOrientation()`
    fn next_orientation(&mut self) {
        self.with_map(|map| {
            let next = match map.get_map_options().north_orientation() {
                NorthOrientation::Upwards => NorthOrientation::Rightwards,
                NorthOrientation::Rightwards => NorthOrientation::Downwards,
                NorthOrientation::Downwards => NorthOrientation::Leftwards,
                _ => NorthOrientation::Upwards,
            };
            map.set_north_orientation(next);
        });
    }

    /// `cycleTileLodProfile(map)`
    fn cycle_tile_lod_profile(&mut self) {
        let map = &mut self.map;
        let defaults = *self.tile_lod_defaults.get_or_insert_with(|| {
            (
                map.get_tile_lod_min_radius(),
                map.get_tile_lod_scale(),
                map.get_tile_lod_pitch_threshold(),
                map.get_tile_lod_mode(),
            )
        });
        self.tile_lod_profile = (self.tile_lod_profile + 1) % 5;
        match self.tile_lod_profile {
            0 => {
                map.set_tile_lod_min_radius(defaults.0);
                map.set_tile_lod_scale(defaults.1);
                map.set_tile_lod_pitch_threshold(defaults.2);
                map.set_tile_lod_mode(defaults.3);
                Log::info(Event::General, "Tile LOD profile: default");
            }
            1 => {
                // When LOD is off we set a maximum PitchThreshold
                map.set_tile_lod_pitch_threshold(std::f64::consts::PI);
                Log::info(Event::General, "Tile LOD profile: disabled");
            }
            2 => {
                map.set_tile_lod_min_radius(2.0);
                map.set_tile_lod_scale(1.5);
                map.set_tile_lod_pitch_threshold(std::f64::consts::PI / 4.0);
                Log::info(Event::General, "Tile LOD profile: reduced");
            }
            3 => {
                map.set_tile_lod_min_radius(1.0);
                map.set_tile_lod_scale(2.0);
                map.set_tile_lod_pitch_threshold(0.0);
                Log::info(Event::General, "Tile LOD profile: aggressive");
            }
            _ => {
                map.set_tile_lod_scale(1.0);
                map.set_tile_lod_pitch_threshold(0.0);
                map.set_tile_lod_mode(TileLodMode::Distance);
                Log::info(Event::General, "Tile LOD profile: distance-based");
            }
        }
        map.trigger_repaint();
    }

    /// `tileLodZoomShift(map, positive)`
    fn tile_lod_zoom_shift(&mut self, positive: bool) {
        self.with_map(|map| {
            const STEP: f64 = 0.25;
            let shift = (map.get_tile_lod_zoom_shift() + if positive { STEP } else { -STEP }).clamp(-2.5, 2.5);
            Log::info(Event::GraphicsBackend, format!("Zoom shift: {shift:.6}"));
            map.set_tile_lod_zoom_shift(shift);
            map.trigger_repaint();
        });
    }

    fn save_readback(&mut self) {
        let Some(image) = self.backend.take_readback() else {
            return;
        };
        let Some(path) = self.readback_path.take() else {
            return;
        };
        let png = mln_image::encode_png(&image);
        match std::fs::write(&path, png) {
            Ok(()) => Log::info(
                Event::General,
                format!(
                    "Read back the presented frame ({}x{}) to {}",
                    image.size.width,
                    image.size.height,
                    path.display()
                ),
            ),
            Err(e) => Log::error(Event::General, format!("Failed to write {}: {e}", path.display())),
        }
    }
}

/// Prints once which layers the renderer draws and which it
/// skips (unported layer types, or sources of unported types).
pub fn report_style(map: &Map) {
    let style = map.get_style();
    let json: serde_json::Value = serde_json::from_str(&style.get_json()).unwrap_or_default();
    let declared_sources: BTreeMap<String, String> = json
        .get("sources")
        .and_then(|s| s.as_object())
        .map(|sources| {
            sources
                .iter()
                .map(|(id, s)| (id.clone(), s.get("type").and_then(|t| t.as_str()).unwrap_or("?").to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let loaded_sources: Vec<String> = style.get_sources().iter().map(|s| s.get_id()).collect();

    let mut drawn: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut skipped_type: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut skipped_source: BTreeMap<String, usize> = BTreeMap::new();
    let impls = style.impl_.get_layer_impls();
    for layer in impls.iter() {
        let type_ = layer.get_type_info().type_;
        // Ask the renderer's layer manager itself, so this summary can't go
        // stale as layer types get ported.
        if mln_renderer::layermanager::create_render_layer(layer).is_none() {
            *skipped_type.entry(type_).or_default() += 1;
        } else if !layer.source.is_empty() && !loaded_sources.contains(&layer.source) {
            let source_type = declared_sources.get(&layer.source).map_or("missing", String::as_str);
            *skipped_source.entry(format!("{type_} on {source_type} source '{}'", layer.source)).or_default() += 1;
        } else {
            *drawn.entry(type_).or_default() += 1;
        }
    }
    let unported_sources: Vec<String> = declared_sources
        .iter()
        .filter(|(id, _)| !loaded_sources.contains(id))
        .map(|(id, t)| format!("{id} ({t})"))
        .collect();
    let list = |m: Vec<(String, usize)>| m.iter().map(|(k, v)| format!("{k} x{v}")).collect::<Vec<_>>().join(", ");
    Log::info(
        Event::General,
        format!(
            "Style \"{}\": {} layers. Rendered: {}. Skipped, layer type not ported: {}. Skipped, source not ported: {}. Unported sources: {}.",
            style.get_name(),
            impls.len(),
            list(drawn.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
            list(skipped_type.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
            list(skipped_source.into_iter().collect()),
            if unported_sources.is_empty() { "none".to_owned() } else { unported_sources.join(", ") },
        ),
    );
}

/// Scripted input (`--gesture-test`).
struct GestureTest {
    step: usize,
    next_at: Option<Instant>,
}

impl View {
    /// Once the style is loaded, feeds a scripted sequence of messages (drag,
    /// scroll, double click, rotate, pitch, trackpad pinch, keys) through the
    /// same handlers as the window's, logs the camera after each, then closes.
    fn run_gesture_test(&mut self) {
        let Some(test) = self.gesture_test.as_mut() else {
            return;
        };
        let now = Instant::now();
        if test.next_at.is_some_and(|t| now < t) {
            return;
        }
        test.next_at = Some(now + std::time::Duration::from_millis(1500));
        let step = test.step;
        test.step += 1;

        let center = ScreenCoordinate::new(f64::from(self.size.width) / 2.0, f64::from(self.size.height) / 2.0);
        let (cx, cy) = (center.x, center.y);
        let none = Modifiers::default();
        let ctrl = Modifiers { ctrl: true, ..none };
        let shift = Modifiers { shift: true, ..none };
        let at = |x: f64, y: f64| ScreenCoordinate::new(x, y);
        let down = |button: Button, modifiers: Modifiers| Input::PointerDown {
            id: 0,
            kind: PointerKind::Mouse,
            button,
            position: center,
            modifiers,
        };
        let drag = |button: Button, modifiers: Modifiers, dx: f64, dy: f64| {
            let mut inputs = vec![Input::PointerMove { id: 0, position: center }, down(button, modifiers)];
            for i in 1..=10 {
                let t = f64::from(i) / 10.0;
                inputs.push(Input::PointerMove { id: 0, position: at(cx + dx * t, cy + dy * t) });
            }
            inputs.push(Input::PointerUp { id: 0, position: at(cx + dx, cy + dy) });
            inputs
        };
        let wheel = |delta: WheelDelta| Input::Wheel { position: center, delta, modifiers: none };
        let pinch = |scale: f64, rotation: f64, phase: PinchPhase| Input::Pinch { center, scale, rotation, phase };
        let key = |code: KeyCode, pressed: bool| (code, pressed);
        let (name, inputs, keys): (&str, Vec<Input>, Vec<(KeyCode, bool)>) = match step {
            0 => ("start", vec![], vec![]),
            1 => ("drag (pan, then glide) by (200, 100)", drag(Button::Primary, none, 200.0, 100.0), vec![]),
            2 => ("scroll up 5 wheel notches (eased)", vec![wheel(WheelDelta::Notches(-1.0)); 5], vec![]),
            3 => ("scroll down 2 wheel notches (eased)", vec![wheel(WheelDelta::Notches(1.0)); 2], vec![]),
            4 => ("scroll a trackpad up 240 px", vec![wheel(WheelDelta::Pixels(-30.0)); 8], vec![]),
            5 => (
                "double click (zoom in, animated)",
                vec![
                    Input::PointerMove { id: 0, position: center },
                    down(Button::Primary, none),
                    Input::PointerUp { id: 0, position: center },
                    down(Button::Primary, none),
                    Input::PointerUp { id: 0, position: center },
                ],
                vec![],
            ),
            6 => ("ctrl+drag (rotate)", drag(Button::Primary, ctrl, 150.0, 0.0), vec![]),
            7 => ("right drag up (tilt)", drag(Button::Secondary, none, 0.0, -120.0), vec![]),
            8 => ("shift+drag (box zoom)", drag(Button::Primary, shift, 200.0, 150.0), vec![]),
            9 => (
                "trackpad pinch (zoom out half a level, turn 20 degrees counterclockwise, then glide)",
                std::iter::once(pinch(1.0, 0.0, PinchPhase::Begin))
                    .chain(std::iter::repeat_n(pinch(2f64.powf(-0.05), -2.0, PinchPhase::Update), 10))
                    .chain(std::iter::once(pinch(1.0, 0.0, PinchPhase::End)))
                    .collect(),
                vec![],
            ),
            10 => ("key Right (pan, animated)", vec![], vec![key(KeyCode::ArrowRight, true)]),
            11 => ("key N (reset north, animated)", vec![], vec![key(KeyCode::KeyN, false)]),
            12 => ("key - (zoom out, animated)", vec![], vec![key(KeyCode::Minus, false)]),
            13 => ("key Tab (debug: tile borders)", vec![], vec![key(KeyCode::Tab, false)]),
            14 => ("key Tab (debug: parse status)", vec![], vec![key(KeyCode::Tab, false)]),
            15 => {
                let _ = self.proxy.send_event(UserEvent::RequestSize(LogicalSize::new(800.0, 600.0)));
                ("window resize to 800x600", vec![], vec![])
            }
            _ => {
                Log::info(Event::General, "Gesture test: done");
                self.gesture_test = None;
                self.quit = true;
                return;
            }
        };
        // Each step's inputs, 16 ms apart (the recognizer measures release
        // velocities and double clicks with their times).
        let mut time = now;
        for input in inputs {
            self.handle(Message::Input { input, at: time });
            time += std::time::Duration::from_millis(16);
        }
        for (code, pressed) in keys {
            self.handle(Message::Key { code, pressed, modifiers: none, cursor: center, at: time });
        }
        let camera = self.map.get_camera_options(None);
        let center = camera.center.unwrap_or_default();
        Log::info(
            Event::General,
            format!(
                "Gesture test step {step} ({name}): center {:.5},{:.5} zoom {:.3} bearing {:.1} pitch {:.1}",
                center.latitude(),
                center.longitude(),
                camera.zoom.unwrap_or(f64::NAN),
                camera.bearing.unwrap_or(f64::NAN),
                camera.pitch.unwrap_or(f64::NAN),
            ),
        );
    }
}
