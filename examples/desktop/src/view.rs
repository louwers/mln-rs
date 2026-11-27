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
// `GestureRecognizer` (mln_engine::gestures), which moves the map's camera and
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
// closes the window.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_engine::camera::{AnimationOptions, BoundOptions, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase};
use mln_engine::geo::ScreenCoordinate;
use mln_engine::gestures::{GestureEvent, GestureRecognizer, Input, Modes, NextTick, PinchPhase, PointerKind};
use mln_engine::map::MapEvent;
use mln_engine::storage::constants::DEFAULT_MAX_CACHE_SIZE;
use mln_runtime::frame_pacer::{FramePacer, RefreshWaiter};
use mln_runtime::{
    CacheDatabaseOperation, Completion, DemandMode, Device, ErrorKind, EventMask, EventType, FrameDemand,
    FrameDisposition, LogicalExtent, Map, MapOptions, Runtime, RuntimeEventKind, RuntimeOptions, ViewportOptions,
};
use mln_style::error::Error;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{KeyCode, ModifiersState};

use crate::Args;
use crate::backend::{self, Kind, WindowSession, WindowSurfaces};
use crate::benchmark::Benchmark;
use crate::host::{Inbox, Message, UserEvent};
use crate::network::Network;
use crate::overlay::Paint;
use crate::pacing::{Pacing, Ready, Refreshes};
use crate::settings::Settings;
use crate::styles::{self, Projection, Style, StyleSource};
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

/// How long a readback waits for its offscreen session to load the map.
const READBACK_TIMEOUT: Duration = Duration::from_secs(30);

/// What the controls show of the map's style.
#[derive(Clone, Debug, Default)]
pub(crate) struct StyleInfo {
    /// The style's own name.
    name: String,
    /// The sources' attributions, as HTML.
    attributions: Vec<String>,
    /// How many layers the style has.
    layers: usize,
    /// How many sources the style has.
    sources: usize,
}

impl StyleInfo {
    /// Reads the style of `map`, on the runtime's thread.
    fn read(map: &mut mln_engine::map::Map) -> StyleInfo {
        let style = map.style();
        let attributions = style
            .get_sources()
            .iter()
            .filter_map(|source| {
                map.registry().source_attribution(source).filter(|attribution| !attribution.is_empty())
            })
            .collect();
        StyleInfo {
            name: style.get_name().to_owned(),
            attributions,
            layers: style.get_layers().len(),
            sources: style.get_sources().len(),
        }
    }
}

/// Opens the map in the window and runs it until the window closes. Fails
/// if the runtime, the map or the window's session can't be made.
pub(crate) fn run(launch: Launch) -> Result<(), String> {
    let Launch { frame_pacer, args, settings, ui, pixel_ratio, size, frame_interval, start, inbox, proxy, .. } = launch;
    #[cfg(not(target_os = "macos"))]
    let surfaces = WindowSurfaces { window: launch.window, cap_frame_rate: args.benchmark.is_none() };
    #[cfg(target_os = "macos")]
    let surfaces = WindowSurfaces { proxy: proxy.clone(), cap_frame_rate: args.benchmark.is_none() };

    let benchmark = args.benchmark.map(|path| Benchmark::new(path, args.laps.unwrap_or(10))).transpose()?;
    // A benchmark reads its fixtures through `asset://` URLs, and nothing
    // else: no cache database, and no network.
    let runtime_options = if let Some(benchmark) = &benchmark {
        RuntimeOptions { asset_path: Some(benchmark.asset_path()), ..RuntimeOptions::default() }
    } else {
        let cache_db = args.cache.clone().map_or_else(|| std::env::temp_dir().join("mbgl-cache.db"), PathBuf::from);
        RuntimeOptions { cache_path: Some(cache_db), ..RuntimeOptions::default() }
    };
    let runtime = Runtime::new(&runtime_options).map_err(|e| e.message)?;
    let network = if benchmark.is_some() { Network::Offline } else { settings.network };
    if network != Network::Online {
        log_failure(network.apply(&runtime));
        if benchmark.is_none() {
            log::warn!("Network: {}", network.label());
        }
    }
    if settings.cache_disabled {
        log_failure(runtime.set_maximum_ambient_cache_size(0));
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
        map: mln_engine::map::MapOptions { size: map_size, pixel_ratio: pixel_ratio as f32, ..Default::default() },
        event_mask: EventMask::NONE | EventType::MapStyleLoaded | EventType::MapIdle | EventType::MapSourceChanged,
        system_fonts: !args.no_system_fonts && benchmark.is_none(),
        ..MapOptions::default()
    };
    let map =
        runtime.create_map(options).and_then(|(map, created)| created.wait().map(|_| map)).map_err(|e| e.message)?;
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
    log_failure(map.set_viewport_options(free_bearing_options(settings.free_bearing)));

    let paint = Rc::new(RefCell::new(Paint::default()));
    let (kind, device) = match args.backend {
        Some(kind) => (kind, None),
        None => backend::default_device().map(|(kind, device)| (kind, Some(device))).map_err(|e| e.message)?,
    };
    let window = WindowSession::attach(kind, device, &map, &surfaces, map_size, Rc::clone(&paint), &inbox)
        .map_err(|e| e.message)?;

    let style = match args.style.clone().or_else(|| std::env::var("MLN_STYLE_URL").ok()) {
        Some(spec) => styles::style_for(&spec),
        None => styles::default_style(),
    };

    let refreshes = match frame_pacer {
        Some(Ok(pacer)) => {
            let woken = Arc::clone(&inbox);
            let waiter: Arc<dyn RefreshWaiter> = Arc::new(move |_| woken.wake_for_refresh());
            Some(DisplayRefreshes { pacer, waiter, inbox: Arc::clone(&inbox) })
        }
        Some(Err(e)) => {
            log::info!("Pacing frames on a timer: {}", e.message);
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
        network,
        cache_disabled: settings.cache_disabled,
        map: map.clone(),
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
        gestures: GestureRecognizer::new(map),
        gestures_animating: false,
        gestures_deadline: None,
        box_zoom: None,
        ui,
        controls: Controls::default(),
        ui_input: None,
        ui_window: egui::RawInput::default(),
        ui_repaint_at: None,
        ui_shapes: Vec::new(),
        paint,
        style_info: StyleInfo::default(),
        styles: styles::styles(),
        style,
        projection: Projection::default(),
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
            log::info!(
                "Exit location: --lat={:.6} --lon={:.6} --zoom={:.6} --bearing={:.6} --pitch={:.6}",
                camera.center.latitude(),
                camera.center.longitude(),
                camera.zoom,
                camera.bearing,
                camera.pitch
            );
        }
        Err(e) => log::warn!("{}", e.message),
    }
    // The session lets go of the window's surface while the window is still
    // open; the map and the runtime go after it.
    view.close_window();
    Ok(())
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
                log::warn!("{}", e.message);
            }
        }),
        Err(e) => log::warn!("{}", e.message),
    }
}

/// Logs the failure of an edit of the map's style that `with_core` ran.
fn log_edit_failure(sent: Result<Completion<Result<(), Error>>, Error>) {
    match sent {
        Ok(completion) => completion.on_complete(|result| {
            if let Err(e) = result.and_then(|edited| edited) {
                log::warn!("{}", e.message);
            }
        }),
        Err(e) => log::warn!("{}", e.message),
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
    network: Network,
    /// Whether the cache database keeps nothing the map loads.
    cache_disabled: bool,
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
    gestures: GestureRecognizer<Map>,
    /// The recognizer eases a scroll, and wants frames.
    gestures_animating: bool,
    /// When the recognizer next needs a tick, outside the frames it asks for.
    gestures_deadline: Option<Instant>,
    /// The box a box-zoom drag draws, which the controls show.
    box_zoom: Option<[ScreenCoordinate; 2]>,

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
    /// The projection the controls picked, which each style that loads
    /// takes.
    projection: Projection,
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
}

impl View {
    /// Milliseconds on the gesture recognizer's clock.
    fn ms(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.start).as_secs_f64() * 1000.0
    }

    /// Loads `style` and names the window after it.
    fn load_style(&mut self, style: Style) {
        let loaded = match &style.source {
            StyleSource::Url(url) => self.map.set_style_url(url),
            StyleSource::Json(json) => self.map.set_style_json(json),
        };
        match loaded {
            Ok(loaded) => loaded.on_complete(|result| match result {
                // Another style's load replaced it.
                Err(e) if e.kind == ErrorKind::Cancelled => {}
                Err(e) => log::error!("Failed to load the style: {}", e.message),
                Ok(_) => {}
            }),
            Err(e) => log::error!("{}", e.message),
        }
        let name = if !style.name.is_empty() {
            style.name.clone()
        } else if let StyleSource::Url(url) = &style.source {
            url.clone()
        } else {
            "custom style".to_owned()
        };
        let _ = self.proxy.send_event(UserEvent::SetTitle(format!("mln-rs: {name}")));
        self.style = style;
        self.style_loaded = false;
    }

    /// Adds the style's terrain to the style that loaded, and draws it with
    /// the projection the controls picked. A command that runs once another
    /// style has started loading changes nothing.
    fn customize_style(&self) {
        let (terrain, projection) = (self.style.terrain.clone(), self.projection);
        if terrain.is_none() && projection == Projection::Style {
            return;
        }
        log_edit_failure(self.map.with_core(move |map| {
            if let Some(terrain) = &terrain {
                styles::add_terrain(map, terrain)?;
            }
            if projection != Projection::Style {
                projection.apply(map)?;
            }
            Ok(())
        }));
    }

    /// Reads what the controls show of the map's style, once the commands
    /// sent before have run.
    fn read_style(&self, loaded: bool) {
        let inbox = Arc::clone(&self.inbox);
        match self.map.with_core(StyleInfo::read) {
            Ok(read) => read.on_complete(move |info| {
                if let Ok(info) = info {
                    inbox.send(Message::Style { info, loaded });
                }
            }),
            Err(e) => log::warn!("{}", e.message),
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
                log::error!("The render session ended: {e}");
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
                    log::error!("The render session failed: {failure}");
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
        let test = self.gesture_test.as_ref().and_then(|t| t.next_at.or(self.style_loaded.then_some(self.start)));
        [self.ui_repaint_at, test, self.quit_at, self.gestures_deadline].into_iter().flatten().min()
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
        let next = self.gestures.tick(ms);
        self.schedule_tick(next);
        self.answer_events();

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
                log::error!("Failed to request a frame: {e}");
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
            Message::Key { code, pressed, repeat, modifiers, at } => self.on_key(code, pressed, repeat, modifiers, at),
            Message::Ui(input) => match &mut self.ui_input {
                Some(pending) => pending.append(input),
                None => self.ui_input = Some(input),
            },
            Message::Resized(size) => self.on_resize(size),
            Message::ScaleFactorChanged(pixel_ratio) => self.on_scale_factor_changed(pixel_ratio),
            Message::Focused { focused } => {
                if !focused && let Some(window) = &self.window {
                    log_failure(window.session().reduce_memory_use());
                }
            }
            Message::Close => self.quit = true,
            Message::Map(MapEvent::StyleLoaded) => {
                self.style_loaded = true;
                self.customize_style();
                self.read_style(true);
            }
            Message::Map(MapEvent::SourceChanged { .. }) => self.read_style(false),
            Message::Map(MapEvent::Idle) => self.idle = true,
            Message::Map(_) => {}
            Message::Style { info, loaded } => {
                if loaded {
                    self.log_loaded_style(&info);
                }
                self.style_info = info;
                self.ui_repaint_at = Some(Instant::now());
            }
            Message::CacheReset => self.clear_renderer(),
        }
    }

    /// Feeds one input to the gesture recognizer, answers its events, and
    /// draws the controls again when a box zoom's box moved.
    fn gesture(&mut self, input: Input, at: Instant) {
        let ms = self.ms(at);
        self.gestures.handle(input, ms);
        self.schedule_tick(self.gestures.next_tick());
        self.answer_events();
        let box_zoom = self.gestures.box_zoom();
        if box_zoom != self.box_zoom {
            self.box_zoom = box_zoom;
            self.ui_repaint_at = Some(Instant::now());
        }
    }

    /// Ticks the recognizer when it asks: every frame while something moves,
    /// or at its deadline.
    fn schedule_tick(&mut self, next: Option<NextTick>) {
        self.gestures_animating = next == Some(NextTick::Frame);
        self.gestures_deadline = match next {
            Some(NextTick::At(ms)) => Some(self.start + Duration::from_secs_f64(ms.max(0.0) / 1000.0)),
            _ => None,
        };
    }

    /// Answers the recognizer's events: a notch ticks the trackpad. The app
    /// has no use for taps.
    fn answer_events(&mut self) {
        for event in self.gestures.take_events() {
            if let GestureEvent::Notch { .. } = event {
                let _ = self.proxy.send_event(UserEvent::Notch);
            }
        }
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

    /// The keys the map takes, which the controls didn't, each repeating
    /// while held: the arrows pan 100 px (Shift: turn 15° and tilt 10°),
    /// `+` and `-` zoom a level (Shift: two), and Escape drops a drag.
    /// Repeats move half as far as the initial press.
    fn on_key(&mut self, code: KeyCode, pressed: bool, repeat: bool, modifiers: ModifiersState, at: Instant) {
        if !pressed {
            return;
        }
        let shift = modifiers.shift_key();
        let levels = if shift { 2.0 } else { 1.0 };
        let (pan, zoom, bearing, pitch) = match (code, shift) {
            (KeyCode::Escape, _) => return self.gesture(Input::PointerCancel { id: 0 }, at),
            (KeyCode::Equal | KeyCode::NumpadAdd, _) => ((0.0, 0.0), levels, 0.0, 0.0),
            (KeyCode::Minus | KeyCode::NumpadSubtract, _) => ((0.0, 0.0), -levels, 0.0, 0.0),
            (KeyCode::ArrowLeft, true) => ((0.0, 0.0), 0.0, -15.0, 0.0),
            (KeyCode::ArrowRight, true) => ((0.0, 0.0), 0.0, 15.0, 0.0),
            (KeyCode::ArrowUp, true) => ((0.0, 0.0), 0.0, 0.0, 10.0),
            (KeyCode::ArrowDown, true) => ((0.0, 0.0), 0.0, 0.0, -10.0),
            (KeyCode::ArrowLeft, false) => ((100.0, 0.0), 0.0, 0.0, 0.0),
            (KeyCode::ArrowRight, false) => ((-100.0, 0.0), 0.0, 0.0, 0.0),
            (KeyCode::ArrowUp, false) => ((0.0, 100.0), 0.0, 0.0, 0.0),
            (KeyCode::ArrowDown, false) => ((0.0, -100.0), 0.0, 0.0, 0.0),
            _ => return,
        };
        let factor = if repeat { 0.5 } else { 1.0 };
        let input = Input::Delta {
            position: self.center(),
            discrete: true,
            pan: ScreenCoordinate::new(pan.0 * factor, pan.1 * factor),
            zoom: zoom * factor,
            bearing: bearing * factor,
            pitch: pitch * factor,
        };
        self.gesture(input, at);
    }

    /// The middle of the map, in logical pixels.
    fn center(&self) -> ScreenCoordinate {
        ScreenCoordinate::new(f64::from(self.size.width) / 2.0, f64::from(self.size.height) / 2.0)
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
            projection: self.projection,
            free_bearing: snapshot.as_ref().is_some_and(|snapshot| snapshot.view.viewport.globe_free_bearing),
            network: self.network,
            cache_disabled: self.cache_disabled,
            backend: self.kind,
            status: &status,
            box_zoom: self.box_zoom,
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
            Action::ZoomIn => self.gesture(zoom_step(self.center(), 1.0), now),
            Action::ZoomOut => self.gesture(zoom_step(self.center(), -1.0), now),
            Action::ResetNorth => {
                let north = CameraUpdate {
                    mode: CameraUpdateMode::Ease,
                    camera: CameraOptions { bearing: Some(0.0), pitch: Some(0.0), ..Default::default() },
                    animation: AnimationOptions::new(Duration::from_millis(500)),
                    phase: GesturePhase::None,
                };
                self.gestures.step_to(&north);
            }
            Action::PickStyle(index) => {
                if let Some(style) = self.styles.get(index).cloned() {
                    self.load_style(style);
                }
            }
            Action::SetProjection(projection) => {
                self.projection = projection;
                log_edit_failure(self.map.with_core(move |map| projection.apply(map)));
            }
            Action::SetFreeBearing(free_bearing) => {
                log_failure(self.map.set_viewport_options(free_bearing_options(free_bearing)));
            }
            Action::SetNetwork(network) => {
                self.network = network;
                log_failure(network.apply(&self.runtime));
            }
            Action::SetCacheDisabled(disabled) => {
                self.cache_disabled = disabled;
                let size = if disabled { 0 } else { DEFAULT_MAX_CACHE_SIZE };
                log_failure(self.runtime.set_maximum_ambient_cache_size(size));
            }
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
            log::error!("Can't switch to {}: {e}", kind.name());
            self.reattach(old, None)
        });
        if let Err(e) = attached {
            log::error!("{}", e.message);
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
        self.kind = kind;
        self.window = Some(window);
        // The controls take egui's textures before the map's next frame.
        self.ui_repaint_at = Some(Instant::now());
        self.dirty = true;
        Ok(())
    }

    /// Empties the cache database, then the renderer's tiles, glyphs and
    /// images, so that the map loads them again from the network.
    fn reset_cache(&mut self) {
        let inbox = Arc::clone(&self.inbox);
        match self.runtime.run_cache_database_operation(CacheDatabaseOperation::Reset) {
            Ok(reset) => reset.on_complete(move |result| {
                match result {
                    Ok(()) => log::info!("Reset the cache"),
                    Err(e) => log::error!("Failed to reset cache: {e}"),
                }
                inbox.send(Message::CacheReset);
            }),
            Err(e) => {
                log::warn!("There is no cache database to reset: {e}");
                self.clear_renderer();
            }
        }
    }

    /// Drops what the renderer has loaded, so that the next frame loads it
    /// again.
    fn clear_renderer(&mut self) {
        if let Some(window) = &self.window {
            log_failure(window.session().clear_data());
            self.dirty = true;
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
            log::info!("{}", self.frame_report);
            self.frames = 0;
            self.frame_time = 0.0;
            self.last_reported = Instant::now();
        }
    }

    /// Logs a one-line summary when a style finishes loading.
    fn log_loaded_style(&self, info: &StyleInfo) {
        let label = if let Some(benchmark) = &self.benchmark {
            format!("\"{}\"", benchmark.name())
        } else if !self.style.name.is_empty() {
            format!("\"{}\"", self.style.name)
        } else if !info.name.is_empty() {
            format!("\"{}\"", info.name)
        } else if let StyleSource::Url(url) = &self.style.source
            && !url.is_empty()
        {
            url.clone()
        } else {
            "(unnamed)".to_owned()
        };
        let plural = |n, word| if n == 1 { format!("{n} {word}") } else { format!("{n} {word}s") };
        log::info!("Loaded style {label} ({}, {})", plural(info.layers, "layer"), plural(info.sources, "source"));
    }

    /// The checks after each frame: the gesture test, the readback and the
    /// quit timer.
    fn after_frame(&mut self) {
        if self.style_loaded {
            self.run_gesture_test();
        }

        if self.readback_path.is_some() && self.idle && self.style_loaded {
            self.read_back();
        }

        if let Some(quit_at) = self.quit_at
            && Instant::now() >= quit_at
        {
            log::info!("--quit-after: closing the window");
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
                let png = mln_engine::image::encode_png(&image);
                match std::fs::write(&path, png) {
                    Ok(()) => log::info!(
                        "Read back the map ({}x{}) to {}",
                        image.size.width,
                        image.size.height,
                        path.display()
                    ),
                    Err(e) => log::error!("Failed to write {}: {e}", path.display()),
                }
            }
            Err(e) => log::error!("Failed to read the map back: {e}"),
        }
        if !self.quit
            && let Err(e) = self.reattach(self.kind, Some(device))
        {
            log::error!("{}", e.message);
            self.quit = true;
        }
    }
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

        let center = self.center();
        let (cx, cy) = (center.x, center.y);
        let none = Modes::default();
        let rotate = Modes { rotate: true, ..none };
        let box_zoom = Modes { box_zoom: true, ..none };
        let at = |x: f64, y: f64| ScreenCoordinate::new(x, y);
        let down = |modes: Modes| Input::PointerDown { id: 0, kind: PointerKind::Mouse, position: center, modes };
        let drag = |modes: Modes, dx: f64, dy: f64| {
            let mut inputs = vec![Input::PointerMove { id: 0, position: center }, down(modes)];
            for i in 1..=10 {
                let t = f64::from(i) / 10.0;
                inputs.push(Input::PointerMove { id: 0, position: at(cx + dx * t, cy + dy * t) });
            }
            inputs.push(Input::PointerUp { id: 0, position: at(cx + dx, cy + dy) });
            inputs
        };
        let wheel = |discrete: bool, zoom: f64| Input::Delta {
            position: center,
            discrete,
            pan: ScreenCoordinate::default(),
            zoom,
            bearing: 0.0,
            pitch: 0.0,
        };
        let pinch = |scale: f64, rotation: f64, phase: PinchPhase| Input::Pinch { center, scale, rotation, phase };
        let (name, inputs, keys, actions): (&str, Vec<Input>, Vec<KeyCode>, Vec<Action>) = match step {
            0 => ("start", vec![], vec![], vec![]),
            1 => ("drag (pan, then glide) by (200, 100)", drag(none, 200.0, 100.0), vec![], vec![]),
            2 => ("scroll up 5 wheel detents (eased)", vec![wheel(true, 1.0 / 3.0); 5], vec![], vec![]),
            3 => ("scroll down 2 wheel detents (eased)", vec![wheel(true, -1.0 / 3.0); 2], vec![], vec![]),
            4 => ("scroll a trackpad up 240 px", vec![wheel(false, 0.1); 8], vec![], vec![]),
            5 => (
                "double click (zoom in, animated)",
                vec![
                    Input::PointerMove { id: 0, position: center },
                    down(none),
                    Input::PointerUp { id: 0, position: center },
                    down(none),
                    Input::PointerUp { id: 0, position: center },
                ],
                vec![],
                vec![],
            ),
            6 => ("ctrl+drag (rotate)", drag(rotate, 150.0, 0.0), vec![], vec![]),
            7 => ("right drag up (tilt)", drag(rotate, 0.0, -120.0), vec![], vec![]),
            8 => ("shift+drag (box zoom)", drag(box_zoom, 200.0, 150.0), vec![], vec![]),
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
            14 => {
                let _ = self.proxy.send_event(UserEvent::RequestSize(LogicalSize::new(800.0, 600.0)));
                ("window resize to 800x600", vec![], vec![], vec![])
            }
            step if Kind::ALL.len() > 1 && (15..15 + Kind::ALL.len()).contains(&step) => {
                let next = self.kind.next();
                ("switch to the next backend", vec![], vec![], vec![Action::SetBackend(next)])
            }
            _ => {
                log::info!("Gesture test: done");
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
            self.handle(Message::Key {
                code,
                pressed: true,
                repeat: false,
                modifiers: ModifiersState::default(),
                at: time,
            });
        }
        for action in actions {
            self.apply(action, time);
        }
        // The camera once the step's commands ran.
        let camera = match self.map.camera().and_then(Completion::wait) {
            Ok(query) => query.camera,
            Err(e) => {
                log::warn!("{}", e.message);
                return;
            }
        };
        log::info!(
            "Gesture test step {step} ({name}): center {:.5},{:.5} zoom {:.3} bearing {:.1} pitch {:.1}",
            camera.center.latitude(),
            camera.center.longitude(),
            camera.zoom,
            camera.bearing,
            camera.pitch
        );
    }
}

/// A discrete delta that zooms `levels` around `position`, as a button or a
/// key does.
fn zoom_step(position: ScreenCoordinate, levels: f64) -> Input {
    Input::Delta { position, discrete: true, pan: ScreenCoordinate::default(), zoom: levels, bearing: 0.0, pitch: 0.0 }
}

/// The viewport options that set whether drags and zooms on the globe turn
/// the bearing, and nothing else.
fn free_bearing_options(free_bearing: bool) -> ViewportOptions {
    ViewportOptions { globe_free_bearing: Some(free_bearing), ..Default::default() }
}
