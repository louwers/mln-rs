// Ports platform/glfw/glfw_view.hpp and platform/glfw/glfw_view.cpp: the
// window, input handling, the frame tick on the RunLoop and the MapObserver.
//
// Structure (C++ GLFWView is one object that the Map observes, the frontend
// invalidates and the GLFW callbacks reach through the window's user
// pointer):
// - `ViewShared` holds what other objects touch while the view is busy
//   (the dirty flag + `glfwPostEmptyEvent`, flags set by the MapObserver);
//   the frontend and `GLFWViewObserver` (the MapObserver) share it.
// - `ViewState` is the rest, behind a RefCell so the frame tick (a
//   `util::Timer` callback on the RunLoop, as in C++) can reach it.
// - GLFW events are polled (`glfwPollEvents`) and then dispatched from the
//   window's event queue, in order, to the ported callbacks (C++ runs the
//   callbacks inside `glfwPollEvents`).
//
// Key bindings: those that map onto ported APIs (see `on_key`); the others
// (annotations, snapshots, queries, custom sources, extrusion/route/feature
// state demos, render-test writer, free camera demo, location indicator,
// rendering stats) log that they are not ported. TODO(port) each.
//
// Port-only additions for checking frames without screenshots: `--readback
// <file.png>` writes the presented frame once the map is idle (or at exit),
// `--quit-after <seconds>` closes the window; the style report prints once
// which layers and sources the renderer skips.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use mln_actor::{RunLoop, Timer};
use mln_gfx::{BackendScope, RendererBackend, ScopeType};
use mln_map::{
    AnimationOptions, BoundOptions, CameraOptions, Map, MapDebugOptions, MapObserver, NorthOrientation,
    RenderFrameStatus, TileLodMode,
};
use mln_util::chrono::Duration;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::glfw_backend::{self, GLFWBackend};
use crate::glfw_renderer_frontend::GLFWRendererFrontend;

/// The part of the view other objects reach while the view is busy.
pub struct ViewShared {
    dirty: Cell<bool>,
    glfw: glfw::Glfw,
    style_loaded: Cell<bool>,
    idle: Cell<bool>,
    pending_title: RefCell<Option<String>>,
}

impl ViewShared {
    /// `invalidate()`
    pub fn invalidate(&self) {
        self.dirty.set(true);
        self.glfw.post_empty_event();
    }

    /// `setWindowTitle(const std::string&)` from code that runs while the
    /// view is busy (the key callbacks); applied on the next frame tick.
    pub fn set_window_title(&self, title: &str) {
        *self.pending_title.borrow_mut() = Some(format!("MapLibre Native (GLFW): {title}"));
        self.glfw.post_empty_event();
    }
}

/// The MapObserver half of `GLFWView`.
pub struct GLFWViewObserver {
    shared: Rc<ViewShared>,
}

impl MapObserver for GLFWViewObserver {
    fn on_did_finish_loading_style(&self) {
        // C++: if (show3DExtrusions) toggle3DExtrusions(show3DExtrusions);
        // TODO(port): 3D extrusions. The port reports the style once.
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

type Callback = Box<dyn FnMut()>;

/// `GLFWView`'s state. Field order is drop order: the backend (and its
/// surface) goes before the window, the window before GLFW.
struct ViewState {
    renderer_frontend: Option<GLFWRendererFrontend>,
    map: Option<Rc<RefCell<Map>>>,
    backend: Box<dyn GLFWBackend>,
    window: glfw::PWindow,
    events: glfw::GlfwReceiver<(f64, glfw::WindowEvent)>,
    glfw: glfw::Glfw,

    benchmark: bool,
    tracking: bool,
    rotating: bool,
    pitching: bool,

    // Frame timer
    frames: i32,
    frame_time: f32,
    last_reported: f64,

    width: i32,
    height: i32,
    pixel_ratio: f32,

    last_x: f64,
    last_y: f64,
    last_click: f64,

    change_style_callback: Option<Callback>,
    pause_resume_callback: Option<Callback>,
    online_status_callback: Option<Callback>,
    reset_database_callback: Option<Callback>,

    // Port-only (see the file comment).
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

/// `GLFWView`
pub struct GLFWView {
    frame_tick: Timer,
    state: Rc<RefCell<ViewState>>,
    shared: Rc<ViewShared>,
    run_loop: RunLoop,
}

const HELP: &str = "
================================================================================

- Press `X` to reset the transform
- Press `N` to reset north
- Press `O` to toggle online connectivity
- Press `S` to cycle through bundled styles
- Press `Z` to cycle through north orientations
- Press `A` to cycle through Mapbox offices in the world + dateline monument
- Press `D` to cycle through camera bounds: inside, crossing IDL at left, crossing IDL at right, and disabled
- Press `U` to toggle pitch bounds
- Press `P` to pause tile requests
- Press `+`/`-` to zoom in/out

- Hold `Control` + mouse drag to rotate
- Hold `Shift` + mouse drag to tilt

- Press `Tab` to cycle through the map debug options
- Press `F6` to cycle through Tile LOD modes
- Press `F7` to lower the zoom level without changing the camera
- Press `F8` to higher the zoom level without changing the camera
- Press `Esc` to quit

(Not ported yet: annotations 1-0/K/L/W/M/C/Q, B, E, F, F1, G, H, I, J, R, T, Y.)

================================================================================
";

impl GLFWView {
    /// `GLFWView(fullscreen, benchmark, resourceOptions, clientOptions)`.
    pub fn new(fullscreen: bool, benchmark: bool) -> Result<GLFWView, String> {
        // C++ declares `mln::util::RunLoop runLoop` as a member.
        let run_loop = RunLoop::new();

        let mut glfw = glfw_backend::init_glfw()?;

        let (mut width, mut height) = (1024i32, 768i32);
        glfw.window_hint(glfw::WindowHint::ClientApi(glfw::ClientApiHint::NoApi));

        let created = if fullscreen {
            glfw.with_primary_monitor(|glfw, monitor| {
                let monitor = monitor?;
                let mode = monitor.get_video_mode()?;
                width = mode.width as i32;
                height = mode.height as i32;
                glfw.create_window(
                    width as u32,
                    height as u32,
                    "MapLibre Native",
                    glfw::WindowMode::FullScreen(monitor),
                )
            })
        } else {
            glfw.create_window(width as u32, height as u32, "MapLibre Native", glfw::WindowMode::Windowed)
        };
        let Some((mut window, events)) = created else {
            return Err("failed to initialize window".to_owned());
        };
        window.set_all_polling(true);

        (width, height) = window.get_size();

        let cap_frame_rate = !benchmark; // disable VSync in benchmark mode
        let backend = glfw_backend::create(&window, cap_frame_rate)?;

        let pixel_ratio = backend.get_framebuffer_size().width as f32 / width as f32;

        // Make sure the window is visible and focused (GLFWWebGPUBackend).
        window.show();
        window.focus();

        print!("{HELP}");

        let shared = Rc::new(ViewShared {
            dirty: Cell::new(false),
            glfw: glfw.clone(),
            style_loaded: Cell::new(false),
            idle: Cell::new(false),
            pending_title: RefCell::new(None),
        });

        Ok(GLFWView {
            frame_tick: Timer::new(),
            state: Rc::new(RefCell::new(ViewState {
                renderer_frontend: None,
                map: None,
                backend,
                window,
                events,
                glfw,
                benchmark,
                tracking: false,
                rotating: false,
                pitching: false,
                frames: 0,
                frame_time: 0.0,
                last_reported: 0.0,
                width,
                height,
                pixel_ratio,
                last_x: 0.0,
                last_y: 0.0,
                last_click: -1.0,
                change_style_callback: None,
                pause_resume_callback: None,
                online_status_callback: None,
                reset_database_callback: None,
                gesture_test: None,
                readback_path: None,
                readback_requested: false,
                quit_at: None,
                style_reported: false,
                next_place: 0,
                tile_lod_defaults: None,
                tile_lod_profile: 0,
            })),
            shared,
            run_loop,
        })
    }

    /// The shared part, for the renderer frontend.
    pub fn shared(&self) -> Rc<ViewShared> {
        self.shared.clone()
    }

    /// The MapObserver to pass to the Map (C++ passes the view itself).
    pub fn map_observer(&self) -> Rc<dyn MapObserver> {
        Rc::new(GLFWViewObserver { shared: self.shared.clone() })
    }

    /// `getPixelRatio()`
    pub fn get_pixel_ratio(&self) -> f32 {
        self.state.borrow().pixel_ratio
    }

    /// `getSize()`
    pub fn get_size(&self) -> Size {
        let state = self.state.borrow();
        Size::new(state.width as u32, state.height as u32)
    }

    /// `getRendererBackend()`
    pub fn get_renderer_backend(&self) -> Rc<dyn RendererBackend> {
        self.state.borrow().backend.get_renderer_backend()
    }

    /// A description of the GPU adapter in use.
    pub fn describe_backend(&self) -> String {
        self.state.borrow().backend.describe()
    }

    /// `setMap(Map*)`
    pub fn set_map(&self, map: Rc<RefCell<Map>>) {
        // TODO(port): map->addAnnotationImage(makeImage("default_marker", 22, 22, 1)).
        self.state.borrow_mut().map = Some(map);
    }

    /// `setRenderFrontend(GLFWRendererFrontend*)`
    pub fn set_render_frontend(&self, frontend: GLFWRendererFrontend) {
        self.state.borrow_mut().renderer_frontend = Some(frontend);
    }

    pub fn set_change_style_callback(&self, callback: impl FnMut() + 'static) {
        self.state.borrow_mut().change_style_callback = Some(Box::new(callback));
    }

    pub fn set_pause_resume_callback(&self, callback: impl FnMut() + 'static) {
        self.state.borrow_mut().pause_resume_callback = Some(Box::new(callback));
    }

    pub fn set_online_status_callback(&self, callback: impl FnMut() + 'static) {
        self.state.borrow_mut().online_status_callback = Some(Box::new(callback));
    }

    pub fn set_reset_cache_callback(&self, callback: impl FnMut() + 'static) {
        self.state.borrow_mut().reset_database_callback = Some(Box::new(callback));
    }

    /// Port-only: write the presented frame to `path` once the map is idle.
    pub fn set_readback(&self, path: PathBuf) {
        self.state.borrow_mut().readback_path = Some(path);
    }

    /// Port-only: once the style is loaded, feed a scripted sequence of
    /// input events (drag, scroll, double click, rotate, pitch, keys) through
    /// the same handlers as GLFW's, log the camera after each, then close.
    pub fn set_gesture_test(&self) {
        self.state.borrow_mut().gesture_test = Some(GestureTest { step: 0, next_at: None });
    }

    /// Port-only: close the window after `seconds`.
    pub fn set_quit_after(&self, seconds: f64) {
        self.state.borrow_mut().quit_at = Some(Instant::now() + std::time::Duration::from_secs_f64(seconds));
    }

    /// `setShouldClose()`
    pub fn set_should_close(&self) {
        self.state.borrow_mut().window.set_should_close(true);
        self.shared.glfw.post_empty_event();
    }

    /// `setWindowTitle(const std::string&)`
    pub fn set_window_title(&self, title: &str) {
        self.state.borrow_mut().window.set_title(&format!("MapLibre Native (GLFW): {title}"));
    }

    /// `invalidate()`
    pub fn invalidate(&self) {
        self.shared.invalidate();
    }

    /// `run()`: a frame tick on the RunLoop polls GLFW and renders, until the
    /// window should close.
    pub fn run(&self) {
        let state = self.state.clone();
        let shared = self.shared.clone();
        let handle = self.run_loop.handle();
        let callback = move || {
            let mut state = state.borrow_mut();
            if state.window.should_close() {
                handle.stop();
                return;
            }

            state.glfw.poll_events();
            state.process_events(&shared);

            state.render(&shared);

            state.port_checks(&shared);
        };

        // Cap frame rate to 60hz if benchmark mode is disabled
        let mut tick_duration = Duration::from_millis(1000 / 60);
        if self.state.borrow().benchmark {
            // 1000 fps is more than enough for benchmarking purposes
            tick_duration = Duration::from_millis(1);
        }
        self.frame_tick.start(Duration::zero(), tick_duration, callback);
        self.run_loop.run();
        self.frame_tick.stop();

        // A readback that never happened (the map never became idle) is
        // taken from one last frame.
        let mut state = self.state.borrow_mut();
        if state.readback_path.is_some() {
            state.backend.request_readback();
            self.shared.dirty.set(true);
            state.render(&self.shared);
            state.save_readback();
        }
    }
}

impl Drop for GLFWView {
    fn drop(&mut self) {
        // ~GLFWView: glfwDestroyWindow(window); glfwTerminate(). The window
        // and GLFW go with the state; the frontend and map references first.
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.renderer_frontend = None;
            state.map = None;
            // The callbacks may hold the map too.
            state.change_style_callback = None;
            state.pause_resume_callback = None;
            state.online_status_callback = None;
            state.reset_database_callback = None;
        }
    }
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

impl ViewState {
    fn with_map(&self, f: impl FnOnce(&mut Map)) {
        if let Some(map) = &self.map {
            f(&mut map.borrow_mut());
        }
    }

    fn process_events(&mut self, shared: &ViewShared) {
        let events: Vec<glfw::WindowEvent> = glfw::flush_messages(&self.events).map(|(_, e)| e).collect();
        self.dispatch_events(events, shared);
    }

    fn dispatch_events(&mut self, events: Vec<glfw::WindowEvent>, shared: &ViewShared) {
        for event in events {
            match event {
                glfw::WindowEvent::Key(key, _, action, mods) => self.on_key(key, action, mods, shared),
                glfw::WindowEvent::Scroll(x, y) => self.on_scroll(x, y),
                glfw::WindowEvent::Size(w, h) => self.on_window_resize(w, h),
                glfw::WindowEvent::FramebufferSize(w, h) => self.on_framebuffer_resize(w, h, shared),
                glfw::WindowEvent::MouseButton(button, action, mods) => self.on_mouse_click(button, action, mods),
                glfw::WindowEvent::CursorPos(x, y) => self.on_mouse_move(x, y),
                glfw::WindowEvent::Focus(focused) => self.on_window_focus(focused),
                _ => {}
            }
        }
    }

    /// `onKey`
    fn on_key(&mut self, key: glfw::Key, action: glfw::Action, mods: glfw::Modifiers, _shared: &ViewShared) {
        use glfw::Key;
        if action == glfw::Action::Release {
            let (last_x, last_y) = (self.last_x, self.last_y);
            match key {
                Key::Escape => self.window.set_should_close(true),
                Key::Tab => self.cycle_debug_options(),
                Key::KpAdd | Key::Equal => self
                    .with_map(|map| map.scale_by(1.1, Some(ScreenCoordinate::new(last_x, last_y)), &animation_ms(500))),
                Key::KpSubtract | Key::Minus => self.with_map(|map| {
                    map.scale_by(1.0 / 1.1, Some(ScreenCoordinate::new(last_x, last_y)), &animation_ms(500))
                }),
                Key::X => {
                    if mods.is_empty() {
                        self.with_map(|map| {
                            map.jump_to(
                                &CameraOptions::default()
                                    .with_center(Some(LatLng::default()))
                                    .with_zoom(Some(0.0))
                                    .with_bearing(Some(0.0))
                                    .with_pitch(Some(0.0)),
                            )
                        })
                    }
                }
                Key::O => {
                    if let Some(cb) = self.online_status_callback.as_mut() {
                        cb();
                    }
                }
                Key::S => {
                    if let Some(cb) = self.change_style_callback.as_mut() {
                        cb();
                    }
                }
                Key::N => {
                    if mods.is_empty() {
                        self.with_map(|map| {
                            map.ease_to(&CameraOptions::default().with_bearing(Some(0.0)), &animation_ms(500))
                        })
                    }
                }
                Key::Z => self.next_orientation(),
                Key::P => {
                    if let Some(cb) = self.pause_resume_callback.as_mut() {
                        cb();
                    }
                }
                Key::I => {
                    if let Some(cb) = self.reset_database_callback.as_mut() {
                        cb();
                    }
                }
                Key::A => {
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
                    self.with_map(|map| map.fly_to(&camera, &AnimationOptions::new(Duration::from_secs(10))));
                }
                Key::D => {
                    // TODO(port): the bounds annotation.
                    Log::warning(Event::General, "Camera bounds cycling is not ported yet");
                }
                Key::U => self.with_map(|map| {
                    let bounds = map.get_bounds();
                    if bounds.min_pitch == Some(mln_math::rad2deg(mln_util::constants::PITCH_MIN))
                        && bounds.max_pitch == Some(mln_math::rad2deg(mln_util::constants::PITCH_MAX))
                    {
                        Log::info(Event::General, "Limiting pitch bounds to [30, 40] degrees");
                        let _ = map.set_bounds(&BoundOptions::default().with_min_pitch(30.0).with_max_pitch(40.0));
                    } else {
                        Log::info(Event::General, "Resetting pitch bounds to [0, 60] degrees");
                        let _ = map.set_bounds(&BoundOptions::default().with_min_pitch(0.0).with_max_pitch(60.0));
                    }
                }),
                Key::F6 => self.cycle_tile_lod_profile(),
                Key::F7 => self.tile_lod_zoom_shift(false),
                Key::F8 => self.tile_lod_zoom_shift(true),
                Key::Q
                | Key::C
                | Key::B
                | Key::K
                | Key::L
                | Key::R
                | Key::E
                | Key::T
                | Key::F
                | Key::F1
                | Key::H
                | Key::J
                | Key::G
                | Key::Y => {
                    Log::warning(Event::General, format!("Key {key:?}: not ported yet"));
                }
                _ => {}
            }
        }
        // TODO(port): W, 1-0, M (annotations) on release/repeat.
    }

    /// `cycleDebugOptions()`
    fn cycle_debug_options(&self) {
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
    fn next_orientation(&self) {
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
        let Some(map) = self.map.clone() else {
            return;
        };
        let mut map = map.borrow_mut();
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
    fn tile_lod_zoom_shift(&self, positive: bool) {
        self.with_map(|map| {
            const STEP: f64 = 0.25;
            let shift = (map.get_tile_lod_zoom_shift() + if positive { STEP } else { -STEP }).clamp(-2.5, 2.5);
            Log::info(Event::GraphicsBackend, format!("Zoom shift: {shift:.6}"));
            map.set_tile_lod_zoom_shift(shift);
            map.trigger_repaint();
        });
    }

    /// `onScroll`
    fn on_scroll(&mut self, _x_offset: f64, y_offset: f64) {
        let wheel_increment = 4.000244140625 / 40.0; // ?
        let pressed = |k| self.window.get_key(k) == glfw::Action::Press;
        let is_shift_active = pressed(glfw::Key::LeftShift) || pressed(glfw::Key::RightShift);
        let is_ctrl_active = pressed(glfw::Key::LeftControl) || pressed(glfw::Key::RightControl);
        let is_wheel = y_offset != 0.0 && y_offset % wheel_increment == 0.0;
        let factor = (if is_wheel { 40.0 } else { 20.0 })
            * if is_shift_active {
                2.0
            } else if is_ctrl_active {
                0.1
            } else {
                1.0
            };
        let delta = y_offset * factor;

        let mut scale_factor = 2.0 / (1.0 + (-delta.abs() / 100.0).exp());

        // Zooming out.
        if delta < 0.0 && scale_factor != 0.0 {
            scale_factor = 1.0 / scale_factor;
        }

        let anchor = ScreenCoordinate::new(self.last_x, self.last_y);
        self.with_map(|map| map.scale_by(scale_factor, Some(anchor), &AnimationOptions::default()));
    }

    /// `onWindowResize`
    fn on_window_resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        let size = Size::new(width.max(0) as u32, height.max(0) as u32);
        self.with_map(|map| {
            // C++ throws for an empty size (a minimized window).
            if let Err(e) = map.set_size(size) {
                Log::warning(Event::General, e);
            }
        });
    }

    /// `onFramebufferResize`
    fn on_framebuffer_resize(&mut self, width: i32, height: i32, shared: &ViewShared) {
        self.backend.set_framebuffer_size(Size::new(width.max(0) as u32, height.max(0) as u32));

        // This is only triggered when the framebuffer is resized, but not the
        // window. It can happen when you move the window between screens with
        // a different pixel ratio. We are forcing a repaint my invalidating the
        // view, which triggers a rerender with the new framebuffer dimensions.
        shared.invalidate();
    }

    /// `onMouseClick`
    fn on_mouse_click(&mut self, button: glfw::MouseButton, action: glfw::Action, modifiers: glfw::Modifiers) {
        let pressed = action == glfw::Action::Press;
        if button == glfw::MouseButton::Button2
            || (button == glfw::MouseButton::Button1 && modifiers.contains(glfw::Modifiers::Control))
        {
            self.rotating = pressed;
            self.with_map(|map| map.set_gesture_in_progress(pressed));
        } else if button == glfw::MouseButton::Button1 && modifiers.contains(glfw::Modifiers::Shift) {
            self.pitching = pressed;
            self.with_map(|map| map.set_gesture_in_progress(pressed));
        } else if button == glfw::MouseButton::Button1 {
            self.tracking = pressed;
            self.with_map(|map| map.set_gesture_in_progress(pressed));

            if action == glfw::Action::Release {
                let now = self.glfw.get_time();
                if now - self.last_click < 0.4 {
                    let anchor = Some(ScreenCoordinate::new(self.last_x, self.last_y));
                    let scale = if modifiers.contains(glfw::Modifiers::Shift) { 0.5 } else { 2.0 };
                    self.with_map(|map| map.scale_by(scale, anchor, &animation_ms(500)));
                }
                self.last_click = now;
            }
        }
    }

    /// `onMouseMove`
    fn on_mouse_move(&mut self, x: f64, y: f64) {
        if self.tracking {
            let dx = x - self.last_x;
            let dy = y - self.last_y;
            if dx != 0.0 || dy != 0.0 {
                self.with_map(|map| {
                    log_error(map.move_by(&ScreenCoordinate::new(dx, dy), &AnimationOptions::default()))
                });
            }
        } else if self.rotating {
            let (first, second) = (ScreenCoordinate::new(self.last_x, self.last_y), ScreenCoordinate::new(x, y));
            self.with_map(|map| map.rotate_by(&first, &second, &AnimationOptions::default()));
        } else if self.pitching {
            let dy = y - self.last_y;
            if dy != 0.0 {
                self.with_map(|map| map.pitch_by(dy / 2.0, &AnimationOptions::default()));
            }
        }
        self.last_x = x;
        self.last_y = y;
        // TODO(port): updatePuckLocation(); the "state-fills" feature-state hover.
    }

    /// `onWindowFocus`
    fn on_window_focus(&mut self, focused: bool) {
        if !focused {
            // Focus lost.
            if let Some(frontend) = &self.renderer_frontend {
                frontend.with_renderer(|renderer| renderer.reduce_memory_use());
            }
        }
    }

    /// `render()`
    fn render(&mut self, shared: &ViewShared) {
        // Port-only perf probe (symbols): `MLN_GLFW_PAN_PX=<px>` pans the map
        // by that many pixels every frame, reversing every 2 s (so it stays
        // over the same area), so the "Frame time" log measures continuous
        // camera movement.
        if let Some(dx) = std::env::var("MLN_GLFW_PAN_PX").ok().and_then(|v| v.parse::<f64>().ok()) {
            let dx = if (self.glfw.get_time() / 2.0) as i64 % 2 == 0 { dx } else { -dx };
            self.with_map(|map| log_error(map.move_by(&ScreenCoordinate::new(dx, 0.0), &AnimationOptions::default())));
            shared.invalidate();
        }
        if shared.dirty.get()
            && let Some(frontend) = self.renderer_frontend.clone()
        {
            // Port: no swapchain texture right now (e.g. a timeout while the
            // compositor is busy): try again on the next tick.
            if !self.backend.begin_frame() {
                return;
            }
            shared.dirty.set(false);
            let started = self.glfw.get_time();

            // TODO(port): animateRouteCallback, updateAnimatedAnnotations.

            let backend = self.backend.get_renderer_backend();
            {
                let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
                frontend.render();
            }

            // TODO(port): updateFreeCameraDemo.

            self.report((1000.0 * (self.glfw.get_time() - started)) as f32);
            if self.benchmark {
                shared.invalidate();
            }
        }
    }

    /// `report(float duration)`
    fn report(&mut self, duration: f32) {
        self.frames += 1;
        self.frame_time += duration;

        let current_time = self.glfw.get_time();
        if current_time - self.last_reported >= 1.0 {
            self.frame_time /= self.frames as f32;
            Log::info(
                Event::Render,
                format!("Frame time: {:.2}ms ({:.2}fps)", self.frame_time, 1000.0 / self.frame_time),
            );
            self.frames = 0;
            self.frame_time = 0.0;
            self.last_reported = current_time;
        }
    }

    /// Port-only checks after each frame tick: the style report, the
    /// readback and the quit timer.
    fn port_checks(&mut self, shared: &ViewShared) {
        if let Some(title) = shared.pending_title.borrow_mut().take() {
            self.window.set_title(&title);
        }

        if self.style_reported {
            self.run_gesture_test(shared);
        }

        if shared.style_loaded.get() && !self.style_reported {
            self.style_reported = true;
            if let Some(map) = &self.map {
                report_style(&map.borrow());
            }
        }

        if self.readback_path.is_some() {
            if self.readback_requested {
                self.save_readback();
            } else if shared.idle.get() && self.style_reported {
                self.backend.request_readback();
                self.readback_requested = true;
                shared.invalidate();
            }
        }

        if let Some(quit_at) = self.quit_at
            && Instant::now() >= quit_at
        {
            Log::info(Event::General, "--quit-after: closing the window");
            self.window.set_should_close(true);
        }
    }

    fn save_readback(&mut self) {
        let Some(image) = self.backend.take_readback() else {
            return;
        };
        let Some(path) = self.readback_path.take() else {
            return;
        };
        let png = mln_platform_default_image::encode_png(&image);
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

/// Port-only: prints once which layers the renderer draws and which it
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

/// Port-only scripted input (see `GLFWView::set_gesture_test`).
struct GestureTest {
    step: usize,
    next_at: Option<Instant>,
}

impl ViewState {
    fn run_gesture_test(&mut self, shared: &ViewShared) {
        use glfw::{Action, Key, Modifiers, MouseButton, WindowEvent};
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

        let (cx, cy) = (f64::from(self.width) / 2.0, f64::from(self.height) / 2.0);
        let none = Modifiers::empty();
        let drag = |modifiers: Modifiers, dx: f64, dy: f64| {
            let mut events = vec![
                WindowEvent::CursorPos(cx, cy),
                WindowEvent::MouseButton(MouseButton::Button1, Action::Press, modifiers),
            ];
            for i in 1..=10 {
                let t = f64::from(i) / 10.0;
                events.push(WindowEvent::CursorPos(cx + dx * t, cy + dy * t));
            }
            events.push(WindowEvent::MouseButton(MouseButton::Button1, Action::Release, modifiers));
            events
        };
        let (name, events): (&str, Vec<WindowEvent>) = match step {
            0 => ("start", vec![]),
            1 => ("drag (pan) by (200, 100)", drag(none, 200.0, 100.0)),
            2 => ("scroll up 5 wheel clicks", (0..5).map(|_| WindowEvent::Scroll(0.0, 1.0)).collect()),
            3 => ("scroll down 2 wheel clicks", (0..2).map(|_| WindowEvent::Scroll(0.0, -1.0)).collect()),
            4 => (
                "double click (zoom in, animated)",
                vec![
                    WindowEvent::CursorPos(cx, cy),
                    WindowEvent::MouseButton(MouseButton::Button1, Action::Press, none),
                    WindowEvent::MouseButton(MouseButton::Button1, Action::Release, none),
                    WindowEvent::MouseButton(MouseButton::Button1, Action::Press, none),
                    WindowEvent::MouseButton(MouseButton::Button1, Action::Release, none),
                ],
            ),
            5 => ("ctrl+drag (rotate)", drag(Modifiers::Control, 150.0, -150.0)),
            6 => ("shift+drag (pitch)", drag(Modifiers::Shift, 0.0, -120.0)),
            7 => ("key N (reset north, animated)", vec![WindowEvent::Key(Key::N, 0, Action::Release, none)]),
            8 => ("key - (zoom out, animated)", vec![WindowEvent::Key(Key::Minus, 0, Action::Release, none)]),
            9 => ("key Tab (debug: tile borders)", vec![WindowEvent::Key(Key::Tab, 0, Action::Release, none)]),
            10 => ("key Tab (debug: parse status)", vec![WindowEvent::Key(Key::Tab, 0, Action::Release, none)]),
            11 => ("window resize to 800x600", {
                self.window.set_size(800, 600);
                vec![]
            }),
            _ => {
                Log::info(Event::General, "Gesture test: done");
                self.gesture_test = None;
                self.window.set_should_close(true);
                return;
            }
        };
        self.dispatch_events(events, shared);
        if let Some(map) = &self.map {
            let camera = map.borrow().get_camera_options(None);
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
}
