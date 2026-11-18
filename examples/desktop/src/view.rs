// The render thread: the map's runtime, the render session that presents the
// map to the window, and the controls over the map (ui.rs).
//
// The map lives on its runtime's thread (mln_runtime), which loads its
// resources and reports its events; this thread sends it commands and reads
// its snapshot. The map's render session renders on this thread, in the
// session's driver work (backend.rs), so that the controls draw over its
// frames here.
//
// The thread sleeps until it has work: a message from the window (host.rs)
// or the runtime, a wake from the render session, or a deadline. Each frame
// takes the messages that came since the last, and the session's frame waits
// for the surface's next texture. Frames start at the display's refreshes,
// or at its refresh interval where the platform reports no refreshes
// (pacing.rs). In benchmark mode (benchmark.rs) they come as fast as the
// surface takes them, each at the next point of the benchmark's camera path,
// without the controls, and input doesn't move the map. While the surface
// takes none, as for a hidden window, they come at the refresh interval.
//
// Mouse, touch, wheel, trackpad and the arrow, `+` and `-` keys go to a
// `GestureRecognizer` (mln_gestures), which moves the map's camera and
// continues releases with the map's momentum.
//
// The controls lay out in egui on this thread, in the frames that have their
// input, a repaint they asked for, or a new frame of the map. egui paints
// them over the map's frame before it is presented (overlay.rs); when only
// the controls change, the session draws the map's last update again under
// them.
//
// For checking frames without screenshots: `--readback <file.png>` writes
// the map once it is idle (or at exit), as an offscreen session on the
// window's device renders it, without the controls; `--quit-after <seconds>`
// closes the window; the style report prints once the style's layers and the
// sources that the map didn't load.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_frame_pacer::{FramePacer, RefreshWaiter};
use mln_gestures::{Button, GestureRecognizer, Input, Key, Modifiers, PinchPhase, PointerKind, WheelDelta};
use mln_map::{
    AnimationOptions, BoundOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, MapDebugOptions,
    MapEvent, RenderMode,
};
use mln_runtime::{
    CacheDatabaseOperation, Completion, DemandMode, Device, ErrorKind, EventMask, EventType, FrameDemand,
    FrameDisposition, LogicalExtent, Map, MapOptions, ResourceRequest, Runtime, RuntimeEventKind, RuntimeOptions,
};
use mln_storage::response::Reason;
use mln_storage::{ResourceError, Response};
use mln_util::error::Error;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::KeyCode;

use crate::Args;
use crate::backend::{self, Kind, WindowSession, WindowSurfaces};
use crate::benchmark::Benchmark;
use crate::host::{Inbox, Message, UserEvent};
use crate::overlay::Paint;
use crate::pacing::{Pacing, Ready, Refreshes};
use crate::settings::Settings;
use crate::styles::{self, Style};
use crate::ui::{Action, Controls, Shown};

/// What the render thread starts with.
pub(crate) struct Launch {
    /// The display's frame pacer, except in benchmark mode.
    pub frame_pacer: Option<Result<Arc<FramePacer>, Error>>,
    pub args: Args,
    pub settings: Settings,
    /// The window, which the render sessions make their surfaces of.
    #[cfg(not(target_os = "macos"))]
    pub window: backend::SessionWindow,
    /// The controls' egui context, which the window's thread feeds.
    pub ui: egui::Context,
    /// The window's scale factor when it opened: the map's first pixel
    /// ratio.
    pub pixel_ratio: f64,
    /// The window's size in physical pixels.
    pub size: PhysicalSize<u32>,
    /// The least time between frames, outside benchmark mode.
    pub frame_interval: Duration,
    /// The epoch of the gesture recognizer's clock.
    pub start: Instant,
    pub inbox: Arc<Inbox>,
    pub proxy: EventLoopProxy<UserEvent>,
}

/// Where the app opens without `--style` or `MLN_STYLE_URL`.
const DEFAULT_STYLE: &str = "https://tiles.openfreemap.org/styles/liberty";

/// How long a readback waits for its offscreen session to load the map.
const READBACK_TIMEOUT: Duration = Duration::from_secs(30);

/// What the controls show of the map's style.
#[derive(Clone, Debug, Default)]
pub(crate) struct StyleInfo {
    /// The style's own name.
    name: String,
    /// The sources' attributions, as HTML.
    attributions: Vec<String>,
    /// Whether the style's projection is the globe.
    globe: bool,
}

impl StyleInfo {
    /// Reads the style of `map`, on the runtime's thread.
    fn read(map: &mut mln_map::Map) -> StyleInfo {
        let style = map.style();
        let attributions = style
            .get_sources()
            .iter()
            .filter_map(|source| source.data().attribution().filter(|attribution| !attribution.is_empty()))
            .collect();
        let projection = style.root_property_json("projection").ok().flatten();
        let globe = projection
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .is_some_and(|projection| projection["type"] == "globe");
        StyleInfo { name: style.get_name().to_owned(), attributions, globe }
    }
}

/// Opens the map in the window and runs it until the window closes.
pub(crate) fn run(launch: Launch) {
    let Launch { frame_pacer, args, settings, ui, pixel_ratio, size, frame_interval, start, inbox, proxy, .. } = launch;
    #[cfg(not(target_os = "macos"))]
    let surfaces = WindowSurfaces { window: launch.window, cap_frame_rate: args.benchmark.is_none() };
    #[cfg(target_os = "macos")]
    let surfaces = WindowSurfaces { proxy: proxy.clone(), cap_frame_rate: args.benchmark.is_none() };

    let benchmark = match args.benchmark.map(|path| Benchmark::new(path, args.laps.unwrap_or(10))).transpose() {
        Ok(benchmark) => benchmark,
        Err(e) => {
            Log::error(Event::Setup, e);
            return;
        }
    };
    // A benchmark reads its fixtures through `asset://` URLs, and nothing
    // else: no cache database, and no network.
    let runtime_options = if let Some(benchmark) = &benchmark {
        RuntimeOptions { asset_path: Some(benchmark.asset_path()), ..RuntimeOptions::default() }
    } else {
        let cache_db = args.cache.clone().unwrap_or_else(|| "/tmp/mbgl-cache.db".to_owned());
        RuntimeOptions { cache_path: Some(cache_db.into()), ..RuntimeOptions::default() }
    };
    let runtime = match Runtime::new(&runtime_options) {
        Ok(runtime) => runtime,
        Err(e) => {
            Log::error(Event::Setup, e.message);
            return;
        }
    };
    if !settings.online || benchmark.is_some() {
        // The cache still serves what it has.
        log_failure(runtime.set_resource_provider(|request: ResourceRequest| {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Connection, "The application is offline")));
            let _ = request.respond(response);
        }));
        Log::warning(Event::Setup, "Application is offline.");
    }
    let events = Arc::clone(&inbox);
    log_failure(runtime.set_event_handler(move |batch| {
        for event in batch {
            if let RuntimeEventKind::Map(event) = &event.kind {
                events.send(Message::Map(event.clone()));
            }
        }
    }));

    let map_size = logical_size(size, pixel_ratio);
    let options = MapOptions {
        map: mln_map::MapOptions { size: map_size, pixel_ratio: pixel_ratio as f32, ..Default::default() },
        event_mask: EventMask::NONE | EventType::MapStyleLoaded | EventType::MapIdle | EventType::MapSourceChanged,
        system_fonts: !args.no_system_fonts && benchmark.is_none(),
        ..MapOptions::default()
    };
    let map = match runtime.create_map(options).and_then(|(map, created)| created.wait().map(|_| map)) {
        Ok(map) => map,
        Err(e) => {
            Log::error(Event::Setup, e.message);
            return;
        }
    };
    log_failure(map.set_bounds(BoundOptions { max_pitch: Some(settings.max_pitch), ..Default::default() }));
    let camera = if let Some(benchmark) = &benchmark {
        benchmark.start()
    } else {
        CameraOptions {
            center: LatLng::new(settings.latitude, settings.longitude).ok(),
            center_altitude: Some(settings.altitude),
            zoom: Some(settings.zoom),
            bearing: Some(settings.bearing),
            pitch: Some(settings.pitch),
            roll: Some(settings.roll),
            fov: Some(settings.fov),
            ..Default::default()
        }
    };
    log_failure(map.update_camera(CameraUpdate { camera, ..Default::default() }));
    log_failure(map.set_debug_flags(MapDebugOptions(settings.debug)));
    log_failure(map.set_globe_free_bearing_enabled(settings.free_bearing));

    let paint = Rc::new(RefCell::new(Paint::default()));
    let window = match WindowSession::attach(args.backend, None, &map, &surfaces, map_size, Rc::clone(&paint), &inbox) {
        Ok(window) => window,
        Err(e) => {
            Log::error(Event::GraphicsBackend, e.message);
            return;
        }
    };
    Log::info(Event::Render, format!("Rendering with {}", window.adapter(&paint.borrow())));

    let style_url = match &args.style {
        Some(style) if !style.contains("://") => format!("file://{style}"),
        Some(style) => style.clone(),
        None => std::env::var("MLN_STYLE_URL").unwrap_or_else(|_| DEFAULT_STYLE.to_owned()),
    };

    let refreshes = match frame_pacer {
        Some(Ok(pacer)) => {
            let woken = Arc::clone(&inbox);
            let waiter: Arc<dyn RefreshWaiter> = Arc::new(move |_| woken.wake_for_refresh());
            Some(DisplayRefreshes { pacer, waiter, inbox: Arc::clone(&inbox) })
        }
        Some(Err(e)) => {
            Log::info(Event::Render, format!("Pacing frames on a timer: {}", e.message));
            None
        }
        None => None,
    };

    let mut view = View {
        inbox,
        proxy,
        start,
        pacing: Pacing::new(refreshes, frame_interval, Instant::now()),
        runtime,
        map,
        surfaces,
        kind: window.kind(),
        window: Some(window),
        demanded: None,
        next_token: 0,
        redraw: false,
        dirty: true,
        target_not_ready: false,
        style_loaded: false,
        idle: false,
        gestures: GestureRecognizer::default(),
        gestures_animating: false,
        ui,
        controls: Controls::default(),
        ui_input: None,
        ui_window: egui::RawInput::default(),
        ui_repaint_at: None,
        ui_shapes: Vec::new(),
        paint,
        style_info: StyleInfo::default(),
        styles: styles::styles(),
        style: styles::style_for(&style_url),
        next_backend: None,
        frame_report: String::new(),
        benchmark,
        pixel_ratio,
        physical_size: size,
        size: map_size,
        quit: false,
        frames: 0,
        frame_time: 0.0,
        last_reported: Instant::now(),
        gesture_test: args.gesture_test.then_some(GestureTest { step: 0, next_at: None }),
        readback_path: args.readback.as_ref().map(PathBuf::from),
        quit_at: args.quit_after.map(|s| Instant::now() + Duration::from_secs_f64(s)),
        style_reported: false,
    };
    if let Some(benchmark) = &view.benchmark {
        log_failure(view.map.set_style_json(benchmark.style()));
        let title = format!("mln-rs: benchmark, {} path", benchmark.name());
        let _ = view.proxy.send_event(UserEvent::SetTitle(title));
    } else {
        let style = view.style.clone();
        view.load_style(style);
    }

    view.run();

    if let Some(benchmark) = &view.benchmark {
        let window = format!(
            "{}x{} at pixel ratio {}, {}",
            view.size.width,
            view.size.height,
            view.pixel_ratio,
            view.window.as_ref().map(|window| window.adapter(&view.paint.borrow())).unwrap_or_default()
        );
        println!("{}", benchmark.summary(&window));
    }

    match view.map.camera().and_then(Completion::wait) {
        Ok(query) => {
            let camera = query.camera;
            Log::info(
                Event::General,
                format!(
                    "Exit location: --lat={:.6} --lon={:.6} --zoom={:.6} --bearing={:.6} --pitch={:.6}",
                    camera.center.latitude(),
                    camera.center.longitude(),
                    camera.zoom,
                    camera.bearing,
                    camera.pitch,
                ),
            );
        }
        Err(e) => Log::warning(Event::General, e.message),
    }
    // The session lets go of the window's surface while the window is still
    // open; the map and the runtime go after it.
    view.close_window();
}

/// A physical size in the map's logical pixels.
fn logical_size(size: PhysicalSize<u32>, pixel_ratio: f64) -> Size {
    let size: LogicalSize<u32> = size.to_logical(pixel_ratio);
    Size::new(size.width, size.height)
}

/// Logs a command's failure: one that couldn't be sent, or that completed
/// with an error.
fn log_failure<T: Send + 'static>(sent: Result<Completion<T>, Error>) {
    match sent {
        Ok(completion) => completion.on_complete(|result| {
            if let Err(e) = result {
                Log::warning(Event::General, e.message);
            }
        }),
        Err(e) => Log::warning(Event::General, e.message),
    }
}

/// The display's refreshes that a frame pacer reports, which wake the
/// render thread through its inbox.
struct DisplayRefreshes {
    pacer: Arc<FramePacer>,
    /// Wakes the render thread.
    waiter: Arc<dyn RefreshWaiter>,
    inbox: Arc<Inbox>,
}

impl Refreshes for DisplayRefreshes {
    fn request(&self) {
        self.pacer.request(&self.waiter);
    }

    fn take(&self) -> bool {
        self.inbox.take_refresh()
    }
}

struct View {
    inbox: Arc<Inbox>,
    proxy: EventLoopProxy<UserEvent>,
    start: Instant,
    pacing: Pacing<DisplayRefreshes>,

    runtime: Runtime,
    map: Map,
    surfaces: WindowSurfaces,
    /// The backend the map renders with.
    kind: Kind,
    /// The map's session on the window. `None` only while the backend
    /// switches or a readback renders.
    window: Option<WindowSession>,
    /// The token of the frame demand whose result hasn't come yet.
    demanded: Option<u64>,
    next_token: u64,
    /// The map has something new to draw.
    redraw: bool,
    /// The view has something new to draw besides the map's updates.
    dirty: bool,
    /// The last frame found no surface texture to draw into, so the next
    /// waits for the frame pacing, even in benchmark mode.
    target_not_ready: bool,
    /// The map loaded the current style.
    style_loaded: bool,
    /// The map has become idle since the view opened.
    idle: bool,
    gestures: GestureRecognizer,
    /// The recognizer eases a scroll, and wants frames.
    gestures_animating: bool,

    ui: egui::Context,
    controls: Controls,
    /// The controls' input since their last frame.
    ui_input: Option<egui::RawInput>,
    /// The window's state as the controls' last input had it, without its
    /// events: a frame without input starts from it, since a default
    /// `RawInput` has no viewport info, and egui would lay the controls out
    /// at one pixel per point, and unfocused.
    ui_window: egui::RawInput,
    /// When the controls asked for their next frame (a tooltip's delay).
    ui_repaint_at: Option<Instant>,
    /// The controls' last frame, to tell whether a frame changed them.
    ui_shapes: Vec<egui::epaint::ClippedShape>,
    paint: Rc<RefCell<Paint>>,
    style_info: StyleInfo,
    styles: Vec<Style>,
    style: Style,
    /// The backend the controls asked for, which the next frame switches to.
    next_backend: Option<Kind>,
    frame_report: String,

    /// The benchmark mode's path and the frames it timed.
    benchmark: Option<Benchmark>,
    pixel_ratio: f64,
    /// The window's size in physical pixels.
    physical_size: PhysicalSize<u32>,
    /// The map's size, in logical pixels.
    size: Size,
    quit: bool,

    // Frame timer
    frames: i32,
    frame_time: f32,
    last_reported: Instant,

    gesture_test: Option<GestureTest>,
    readback_path: Option<PathBuf>,
    quit_at: Option<Instant>,
    style_reported: bool,
}

impl View {
    /// Milliseconds on the gesture recognizer's clock.
    fn ms(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.start).as_secs_f64() * 1000.0
    }

    /// Loads `style` and names the window after it.
    fn load_style(&mut self, style: Style) {
        match self.map.set_style_url(&style.url) {
            Ok(loaded) => loaded.on_complete(|result| match result {
                // Another style's load replaced it.
                Err(e) if e.kind == ErrorKind::Cancelled => {}
                Err(e) => Log::error(Event::General, format!("Failed to load the style: {}", e.message)),
                Ok(_) => {}
            }),
            Err(e) => Log::error(Event::General, e.message),
        }
        let name = if style.name.is_empty() { style.url.clone() } else { style.name.clone() };
        let _ = self.proxy.send_event(UserEvent::SetTitle(format!("mln-rs: {name}")));
        self.style = style;
        self.style_reported = false;
        self.style_loaded = false;
    }

    /// Reads what the controls show of the map's style, once the commands
    /// sent before have run.
    fn read_style(&self) {
        let inbox = Arc::clone(&self.inbox);
        match self.map.with_core(StyleInfo::read) {
            Ok(read) => read.on_complete(move |info| {
                if let Ok(info) = info {
                    inbox.send(Message::Style(info));
                }
            }),
            Err(e) => Log::warning(Event::General, e.message),
        }
    }

    /// Frames and work until the window closes. A readback that never
    /// happened (the map never became idle) renders at exit.
    fn run(&mut self) {
        while !self.quit {
            self.service();
            if self.quit {
                break;
            }
            let now = Instant::now();
            if !self.frame_due(now) {
                self.inbox.wait(self.next_deadline());
                continue;
            }
            if (self.benchmark.is_none() || self.target_not_ready)
                && let Ready::Wait(until) = self.pacing.ready(now)
            {
                self.inbox.wait(Some(until));
                continue;
            }
            self.frame();
        }
        if self.readback_path.is_some() {
            self.read_back();
        }
    }

    /// Runs the render session's work that is ready, and takes its frame
    /// results.
    fn service(&mut self) {
        self.redraw |= self.inbox.take_redraw();
        let Some(window) = &self.window else { return };
        let started = Instant::now();
        let results = window.service().and_then(|()| window.session().drain_frame_results());
        let elapsed = started.elapsed();
        let results = match results {
            Ok(results) => results,
            Err(e) => {
                Log::error(Event::Render, format!("The render session ended: {e}"));
                self.quit = true;
                return;
            }
        };
        let mut rendered = false;
        let now = Instant::now();
        for result in results {
            if result.token.is_some() && result.token == self.demanded {
                self.demanded = None;
            }
            match result.disposition {
                FrameDisposition::Rendered => {
                    rendered = true;
                    self.target_not_ready = false;
                    if let Some(benchmark) = &mut self.benchmark
                        && benchmark.rendered(result.token, now, result.mode == RenderMode::Full, result.needs_repaint)
                    {
                        self.quit = true;
                    }
                }
                // No texture right now (e.g. a timeout while the compositor
                // is busy, or a hidden window): try again on the next frame.
                FrameDisposition::TargetNotReady => {
                    self.dirty = true;
                    self.target_not_ready = true;
                }
                FrameDisposition::Failed => {
                    let failure = window.session().snapshot().ok().and_then(|snapshot| snapshot.failure);
                    let failure = failure.map_or_else(|| "unknown".to_owned(), |e| e.message);
                    Log::error(Event::Render, format!("The render session failed: {failure}"));
                    self.quit = true;
                }
                FrameDisposition::NoUpdate | FrameDisposition::Superseded => {}
            }
        }
        if rendered {
            self.report((1000.0 * elapsed.as_secs_f64()) as f32);
        }
    }

    /// Whether the map has a frame to draw: a new update, or what the view
    /// marked.
    fn needs_frame(&self) -> bool {
        self.redraw || self.dirty || self.benchmark.is_some()
    }

    /// Whether a frame has something to do.
    fn frame_due(&self, now: Instant) -> bool {
        self.needs_frame()
            || self.gestures_animating
            || !self.inbox.is_empty()
            || self.ui_input.is_some()
            || self.next_deadline().is_some_and(|at| at <= now)
            || (self.readback_path.is_some() && self.idle)
    }

    /// When a frame is due without input: the controls' repaint, the gesture
    /// test's next step, or the quit timer.
    fn next_deadline(&self) -> Option<Instant> {
        let test = self.gesture_test.as_ref().and_then(|t| t.next_at.or(self.style_reported.then_some(self.start)));
        [self.ui_repaint_at, test, self.quit_at].into_iter().flatten().min()
    }

    /// One frame: the messages that came since the last, the recognizer's
    /// eases, the controls, and the map's frame.
    fn frame(&mut self) {
        if let Some(kind) = self.next_backend.take() {
            self.switch_backend(kind);
        }
        for message in self.inbox.take() {
            self.handle(message);
        }
        let now = Instant::now();
        let ms = self.ms(now);
        self.gestures_animating = self.gestures.frame(&mut self.map, ms);

        self.redraw |= self.inbox.take_redraw();
        let controls_due =
            self.ui_input.is_some() || self.needs_frame() || self.ui_repaint_at.is_some_and(|at| at <= now);
        if controls_due && self.benchmark.is_none() {
            self.controls_frame(now);
        }
        self.demand_frame();
        self.service();
        self.after_frame();
    }

    /// Asks the session for a frame when the map or the controls have
    /// something new, unless a frame is on its way: one of the controls'
    /// changes draws the map's last update again.
    fn demand_frame(&mut self) {
        if self.demanded.is_some() || !self.needs_frame() {
            return;
        }
        let Some(window) = &self.window else { return };
        let mode = if self.dirty || self.benchmark.is_some() { DemandMode::Force } else { DemandMode::IfChanged };
        self.next_token += 1;
        // The demand waits for the commands sent before it, so its frame
        // shows the path's next camera.
        if let Some(camera) = self.benchmark.as_mut().and_then(|benchmark| benchmark.next_camera(self.next_token)) {
            log_failure(self.map.update_camera(CameraUpdate { camera, ..Default::default() }));
        }
        match window.session().demand_frame(FrameDemand::new(mode, self.next_token)) {
            Ok(()) => {
                self.demanded = Some(self.next_token);
                self.redraw = false;
                self.dirty = false;
            }
            Err(e) => {
                Log::error(Event::Render, format!("The render session takes no frames: {e}"));
                self.quit = true;
            }
        }
    }

    /// Applies one message from the window or the runtime.
    fn handle(&mut self, message: Message) {
        // A benchmark's camera follows its path, and it has no controls.
        if self.benchmark.is_some() && matches!(message, Message::Input { .. } | Message::Key { .. } | Message::Ui(_)) {
            return;
        }
        match message {
            Message::Input { input, at } => self.gesture(input, at),
            Message::Key { code, pressed, modifiers, at } => self.on_key(code, pressed, modifiers, at),
            Message::Ui(input) => match &mut self.ui_input {
                Some(pending) => pending.append(input),
                None => self.ui_input = Some(input),
            },
            Message::Resized(size) => self.on_resize(size),
            Message::ScaleFactorChanged(pixel_ratio) => self.on_scale_factor_changed(pixel_ratio),
            Message::Focused { focused, at } => {
                if !focused {
                    // A drag the window can't see the end of ends here.
                    self.gesture(Input::Blur, at);
                    if let Some(window) = &self.window {
                        log_failure(window.session().reduce_memory_use());
                    }
                }
            }
            Message::Close => self.quit = true,
            Message::Map(MapEvent::StyleLoaded) => {
                self.style_loaded = true;
                self.read_style();
            }
            Message::Map(MapEvent::SourceChanged { .. }) => self.read_style(),
            Message::Map(MapEvent::Idle) => self.idle = true,
            Message::Map(_) => {}
            Message::Style(info) => {
                self.style_info = info;
                self.ui_repaint_at = Some(Instant::now());
            }
        }
    }

    /// Feeds one input to the gesture recognizer.
    fn gesture(&mut self, input: Input, at: Instant) {
        let at = self.ms(at);
        self.gestures.handle(&mut self.map, input, at);
        // The app has no use for clicks.
        self.gestures.take_events();
    }

    fn on_resize(&mut self, size: PhysicalSize<u32>) {
        self.physical_size = size;
        self.size = logical_size(size, self.pixel_ratio);
        if let Some(window) = &self.window {
            // An empty size (a minimized window) fails.
            log_failure(window.session().resize(LogicalExtent { size: self.size, scale_factor: self.pixel_ratio }));
        }
        self.dirty = true;
    }

    /// Draws the map at `pixel_ratio` from now on, keeping its tiles, and
    /// sizes it in logical pixels of the new ratio. The window's `Resized`
    /// that follows brings the new physical size, where it changed.
    fn on_scale_factor_changed(&mut self, pixel_ratio: f64) {
        self.pixel_ratio = pixel_ratio;
        self.on_resize(self.physical_size);
    }

    /// The keys the map takes, which the controls didn't: the arrows pan
    /// (Shift: rotate and tilt), `+` and `-` zoom, each repeating while held,
    /// and Escape ends a box zoom.
    fn on_key(&mut self, code: KeyCode, pressed: bool, modifiers: Modifiers, at: Instant) {
        let key = match code {
            KeyCode::ArrowLeft => Key::Left,
            KeyCode::ArrowRight => Key::Right,
            KeyCode::ArrowUp => Key::Up,
            KeyCode::ArrowDown => Key::Down,
            KeyCode::Equal | KeyCode::NumpadAdd => Key::ZoomIn,
            KeyCode::Minus | KeyCode::NumpadSubtract => Key::ZoomOut,
            KeyCode::Escape => Key::Escape,
            _ => return,
        };
        if pressed {
            self.gesture(Input::Key { key, modifiers }, at);
        }
    }

    /// The controls' frame: lays them out with their input, carries out what
    /// they ask, and draws the map again under them when they changed.
    fn controls_frame(&mut self, now: Instant) {
        let mut input = self.ui_window.clone();
        if let Some(newer) = self.ui_input.take() {
            input.append(newer);
            self.ui_window = input.clone();
            self.ui_window.events.clear();
            self.ui_window.hovered_files.clear();
            self.ui_window.dropped_files.clear();
            for viewport in self.ui_window.viewports.values_mut() {
                viewport.events.clear();
            }
        }
        input.time = Some(now.saturating_duration_since(self.start).as_secs_f64());

        let snapshot = self.map.snapshot().ok();
        let camera = snapshot.as_ref().map(|snapshot| snapshot.view.camera);
        let adapter = self.window.as_ref().map(|window| window.adapter(&self.paint.borrow())).unwrap_or_default();
        let status = [adapter, self.frame_report.clone()];
        let shown = Shown {
            styles: &self.styles,
            style: &self.style,
            style_name: &self.style_info.name,
            loading: !snapshot.as_ref().is_some_and(|snapshot| snapshot.fully_loaded),
            bearing: camera.map_or(0.0, |camera| camera.bearing),
            pitch: camera.map_or(0.0, |camera| camera.pitch),
            attributions: &self.style_info.attributions,
            debug: snapshot.as_ref().map_or_else(MapDebugOptions::default, |snapshot| snapshot.debug),
            globe: self.style_info.globe,
            free_bearing: snapshot.as_ref().is_some_and(|snapshot| snapshot.globe_free_bearing),
            backend: self.kind,
            status: &status,
        };
        let mut actions = Vec::new();
        let controls = &mut self.controls;
        let output = self.ui.run_pass(input, |ctx| actions = controls.show(ctx, &shown));

        let _ = self.proxy.send_event(UserEvent::Ui(Box::new(output.platform_output)));
        self.ui_repaint_at = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .filter(|delay| *delay < Duration::from_secs(3600))
            .map(|delay| now + delay);
        if output.shapes != self.ui_shapes || !output.textures_delta.is_empty() {
            let primitives = self.ui.tessellate(output.shapes.clone(), output.pixels_per_point);
            let mut paint = self.paint.borrow_mut();
            paint.primitives = primitives;
            paint.pixels_per_point = output.pixels_per_point;
            paint.textures.append(output.textures_delta);
            self.ui_shapes = output.shapes;
            self.dirty = true;
        }
        for action in actions {
            self.apply(action, now);
        }
    }

    /// Carries out what the controls ask.
    fn apply(&mut self, action: Action, now: Instant) {
        match action {
            Action::ZoomIn => self.gesture(Input::Key { key: Key::ZoomIn, modifiers: Modifiers::default() }, now),
            Action::ZoomOut => self.gesture(Input::Key { key: Key::ZoomOut, modifiers: Modifiers::default() }, now),
            Action::ResetNorth => {
                let north = CameraUpdate {
                    mode: CameraUpdateMode::Ease,
                    camera: CameraOptions { bearing: Some(0.0), pitch: Some(0.0), ..Default::default() },
                    animation: AnimationOptions::new(Duration::from_millis(500)),
                    phase: GesturePhase::None,
                };
                self.gestures.step_to(&mut self.map, &north);
            }
            Action::PickStyle(index) => {
                if let Some(style) = self.styles.get(index).cloned() {
                    self.load_style(style);
                }
            }
            Action::SetDebug(debug) => log_failure(self.map.set_debug_flags(debug)),
            Action::SetGlobe(globe) => {
                let projection = if globe { r#"{"type":"globe"}"# } else { r#"{"type":"mercator"}"# };
                log_failure(self.map.set_style_root_property("projection", projection));
                self.read_style();
            }
            Action::SetFreeBearing(free_bearing) => log_failure(self.map.set_globe_free_bearing_enabled(free_bearing)),
            Action::ResetCache => self.reset_cache(),
            Action::SetBackend(kind) => {
                self.next_backend = Some(kind);
                self.dirty = true;
            }
        }
    }

    /// Renders with `kind` from now on: a new session on a device of its
    /// own, which loads the tiles again (from the cache).
    fn switch_backend(&mut self, kind: Kind) {
        let old = self.kind;
        if old == kind {
            return;
        }
        // The old session goes first: the window takes one session's frames
        // at a time. When the new one fails, the old kind takes the window
        // back.
        self.close_window();
        let attached = self.reattach(kind, None).or_else(|e| {
            Log::error(Event::GraphicsBackend, format!("Can't switch to {}: {e}", kind.name()));
            self.reattach(old, None)
        });
        if let Err(e) = attached {
            Log::error(Event::GraphicsBackend, e.message);
            self.quit = true;
        }
    }

    /// Releases the window's session.
    fn close_window(&mut self) {
        if let Some(window) = self.window.take() {
            window.close();
        }
        self.demanded = None;
    }

    /// Attaches a session of `kind` to the window again, on `device` or a
    /// new one, after another session let go of it.
    fn reattach(&mut self, kind: Kind, device: Option<Device>) -> Result<(), Error> {
        let (map, surfaces, size, paint) = (&self.map, &self.surfaces, self.size, Rc::clone(&self.paint));
        let window = WindowSession::attach(kind, device, map, surfaces, size, paint, &self.inbox)?;
        {
            // The new overlay has none of egui's textures, and names its GPU
            // again.
            let mut paint = self.paint.borrow_mut();
            paint.resend_textures(&self.ui);
            paint.renderer = None;
        }
        Log::info(Event::Render, format!("Rendering with {}", window.adapter(&self.paint.borrow())));
        self.kind = kind;
        self.window = Some(window);
        // The controls take egui's textures before the map's next frame.
        self.ui_repaint_at = Some(Instant::now());
        self.dirty = true;
        Ok(())
    }

    /// Empties the cache database.
    fn reset_cache(&self) {
        match self.runtime.run_cache_database_operation(CacheDatabaseOperation::Reset) {
            Ok(reset) => reset.on_complete(|result| match result {
                Ok(()) => Log::info(Event::Database, "Reset the cache"),
                Err(e) => Log::error(Event::Database, format!("Failed to reset cache: {e}")),
            }),
            Err(e) => Log::warning(Event::Database, format!("There is no cache database to reset: {e}")),
        }
    }

    /// `report(float duration)`: the time the session took for a frame,
    /// waiting for the surface's texture included, logged and shown in the
    /// controls' panel once a second.
    fn report(&mut self, duration: f32) {
        self.frames += 1;
        self.frame_time += duration;
        let elapsed = self.last_reported.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let fps = f64::from(self.frames) / elapsed;
            self.frame_time /= self.frames as f32;
            self.frame_report = format!("Frame time: {:.2} ms ({fps:.0} fps)", self.frame_time);
            Log::info(Event::Render, &self.frame_report);
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

        if self.style_loaded && !self.style_reported {
            self.style_reported = true;
            report_style(&self.map);
        }

        if self.readback_path.is_some() && self.idle && self.style_reported {
            self.read_back();
        }

        if let Some(quit_at) = self.quit_at
            && Instant::now() >= quit_at
        {
            Log::info(Event::General, "--quit-after: closing the window");
            self.quit = true;
        }
    }

    /// Writes the map to the readback's file, as an offscreen session on the
    /// window's device renders it once everything has loaded. The window's
    /// session gives the map up meanwhile, and a new one takes it back unless
    /// the window is closing.
    fn read_back(&mut self) {
        let (Some(path), Some(window)) = (self.readback_path.take(), self.window.as_ref()) else {
            return;
        };
        let device = window.device().clone();
        self.close_window();
        match backend::read_offscreen(&self.map, device.clone(), self.size, &self.inbox, READBACK_TIMEOUT) {
            Ok(image) => {
                let png = mln_image::encode_png(&image);
                match std::fs::write(&path, png) {
                    Ok(()) => Log::info(
                        Event::General,
                        format!("Read back the map ({}x{}) to {}", image.size.width, image.size.height, path.display()),
                    ),
                    Err(e) => Log::error(Event::General, format!("Failed to write {}: {e}", path.display())),
                }
            }
            Err(e) => Log::error(Event::General, format!("Failed to read the map back: {e}")),
        }
        if !self.quit
            && let Err(e) = self.reattach(self.kind, Some(device))
        {
            Log::error(Event::GraphicsBackend, e.message);
            self.quit = true;
        }
    }
}

/// Prints once how many layers of each type the style has, and which
/// sources the map didn't load, with the layers on them.
fn report_style(map: &Map) {
    let queried = (|| -> Result<_, Error> {
        let (json, layers, sources) = (map.loaded_style_json()?, map.style_layers()?, map.style_sources()?);
        Ok((json.wait()?, layers.wait()?, sources.wait()?))
    })();
    let (json, layers, sources) = match queried {
        Ok(queried) => queried,
        Err(e) => {
            Log::warning(Event::General, format!("Can't report on the style: {e}"));
            return;
        }
    };
    let json: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
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
    let loaded_sources: Vec<String> = sources.into_iter().map(|source| source.id).collect();

    let mut by_type: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut unloaded_source: BTreeMap<String, usize> = BTreeMap::new();
    for layer in &layers {
        *by_type.entry(layer.layer_type).or_default() += 1;
        if let Some(source) = layer.source_id.as_ref().filter(|source| !loaded_sources.contains(source)) {
            let source_type = declared_sources.get(source).map_or("missing", String::as_str);
            *unloaded_source.entry(format!("{} on {source_type} source '{source}'", layer.layer_type)).or_default() +=
                1;
        }
    }
    let unloaded_sources: Vec<String> = declared_sources
        .iter()
        .filter(|(id, _)| !loaded_sources.contains(id))
        .map(|(id, t)| format!("{id} ({t})"))
        .collect();
    let list = |m: Vec<(String, usize)>| {
        if m.is_empty() {
            return "none".to_owned();
        }
        m.iter().map(|(k, v)| format!("{k} x{v}")).collect::<Vec<_>>().join(", ")
    };
    Log::info(
        Event::General,
        format!(
            "Style \"{}\": {} layers: {}. On sources the map didn't load: {}. Sources the map didn't load: {}.",
            json.get("name").and_then(|name| name.as_str()).unwrap_or_default(),
            layers.len(),
            list(by_type.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()),
            list(unloaded_source.into_iter().collect()),
            if unloaded_sources.is_empty() { "none".to_owned() } else { unloaded_sources.join(", ") },
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
    /// same handlers as the window's, and the controls' actions, logs the
    /// camera after each, resizes the window, switches through the
    /// platform's backends back to the first where there are several, then
    /// closes.
    fn run_gesture_test(&mut self) {
        let Some(test) = self.gesture_test.as_mut() else {
            return;
        };
        let now = Instant::now();
        if test.next_at.is_some_and(|t| now < t) {
            return;
        }
        test.next_at = Some(now + Duration::from_millis(1500));
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
        let (name, inputs, keys, actions): (&str, Vec<Input>, Vec<KeyCode>, Vec<Action>) = match step {
            0 => ("start", vec![], vec![], vec![]),
            1 => ("drag (pan, then glide) by (200, 100)", drag(Button::Primary, none, 200.0, 100.0), vec![], vec![]),
            2 => ("scroll up 5 wheel notches (eased)", vec![wheel(WheelDelta::Notches(-1.0)); 5], vec![], vec![]),
            3 => ("scroll down 2 wheel notches (eased)", vec![wheel(WheelDelta::Notches(1.0)); 2], vec![], vec![]),
            4 => ("scroll a trackpad up 240 px", vec![wheel(WheelDelta::Pixels(-30.0)); 8], vec![], vec![]),
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
                vec![],
            ),
            6 => ("ctrl+drag (rotate)", drag(Button::Primary, ctrl, 150.0, 0.0), vec![], vec![]),
            7 => ("right drag up (tilt)", drag(Button::Secondary, none, 0.0, -120.0), vec![], vec![]),
            8 => ("shift+drag (box zoom)", drag(Button::Primary, shift, 200.0, 150.0), vec![], vec![]),
            9 => (
                "trackpad pinch (zoom out half a level, turn 20 degrees counterclockwise, then glide)",
                std::iter::once(pinch(1.0, 0.0, PinchPhase::Begin))
                    .chain(std::iter::repeat_n(pinch(2f64.powf(-0.05), -2.0, PinchPhase::Update), 10))
                    .chain(std::iter::once(pinch(1.0, 0.0, PinchPhase::End)))
                    .collect(),
                vec![],
                vec![],
            ),
            10 => ("key Right (pan, animated)", vec![], vec![KeyCode::ArrowRight], vec![]),
            11 => ("compass (reset north and tilt, animated)", vec![], vec![], vec![Action::ResetNorth]),
            12 => ("key - (zoom out, animated)", vec![], vec![KeyCode::Minus], vec![]),
            13 => ("zoom in button (animated)", vec![], vec![], vec![Action::ZoomIn]),
            14 => (
                "debug overlays: tile borders and parse status",
                vec![],
                vec![],
                vec![Action::SetDebug(MapDebugOptions::TILE_BORDERS | MapDebugOptions::PARSE_STATUS)],
            ),
            15 => {
                let _ = self.proxy.send_event(UserEvent::RequestSize(LogicalSize::new(800.0, 600.0)));
                ("window resize to 800x600", vec![], vec![], vec![])
            }
            step if Kind::ALL.len() > 1 && (16..16 + Kind::ALL.len()).contains(&step) => {
                let next = self.kind.next();
                ("switch to the next backend", vec![], vec![], vec![Action::SetBackend(next)])
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
            time += Duration::from_millis(16);
        }
        for code in keys {
            self.handle(Message::Key { code, pressed: true, modifiers: none, at: time });
        }
        for action in actions {
            self.apply(action, time);
        }
        // The camera once the step's commands ran.
        let camera = match self.map.camera().and_then(Completion::wait) {
            Ok(query) => query.camera,
            Err(e) => {
                Log::warning(Event::General, e.message);
                return;
            }
        };
        Log::info(
            Event::General,
            format!(
                "Gesture test step {step} ({name}): center {:.5},{:.5} zoom {:.3} bearing {:.1} pitch {:.1}",
                camera.center.latitude(),
                camera.center.longitude(),
                camera.zoom,
                camera.bearing,
                camera.pitch,
            ),
        );
    }
}
