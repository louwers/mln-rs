// The window: winit's event loop on the main thread, and the render thread
// (view.rs) that drives the map and presents it.
//
// The main thread only turns window events into `Message`s for the render
// thread and carries out what the render thread asks of the window (on
// macOS a surface, its title, its size, egui's cursor and links). Frames
// wait for the display on the render thread: on macOS, waiting in
// `-[CAMetalLayer nextDrawable]` on the main thread holds up the window's
// input with it, and a trackpad's first moves arrive ~100 ms late
// (rust-windowing/winit#1737).
//
// Every event goes to egui (egui-winit), whose input the render thread's
// controls (ui.rs) take. The map gets the pointer's events where the
// controls weren't, as of egui's last frame: a press over a control is the
// control's, and a press on the map keeps the pointer the map's until it is
// released, wherever it moves.
//
// Positions reach the map in its logical pixels: physical pixels divided by
// the pixel ratio the map was created with. The map keeps that ratio when the
// window moves to a display with another scale factor; egui follows the
// window's.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mln_engine::geo::ScreenCoordinate;
use mln_engine::gestures::{Input, Modes, PinchPhase, PointerKind};
use mln_engine::map::MapEvent;
use mln_runtime::frame_pacer::FramePacer;
use mln_style::error::Error;
use parking_lot::{Condvar, Mutex};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::Args;
#[cfg(target_os = "macos")]
use crate::metal_layer::MetalLayer;
use crate::settings::Settings;
use crate::view::{self, Launch};

/// A frame pacer of `window`'s display. Runs on the main thread.
#[cfg_attr(target_os = "macos", expect(unsafe_code, reason = "the window's view is a pointer to an NSView"))]
fn frame_pacer(window: &Window) -> Result<Arc<FramePacer>, Error> {
    #[cfg(target_os = "macos")]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let handle = window.window_handle().map_err(|e| Error::native(format!("the window has no handle: {e}")))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(Error::native("the window is not an AppKit window"));
        };
        // SAFETY: the handle's view is the window's NSView, and this runs on
        // the main thread with the window alive.
        unsafe { FramePacer::for_view(handle.ns_view, None) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        FramePacer::new(None)
    }
}

/// The window's size before it opens, in logical pixels (C++'s 1024x768).
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1024.0, 768.0);

/// What the main thread and the runtime hand the render thread, in the order
/// it happened.
pub(crate) enum Message {
    /// Input for the gesture recognizer, in the map's logical pixels.
    Input {
        input: Input,
        at: Instant,
    },
    /// A key that the controls didn't take went down (or repeats) or up.
    Key {
        code: KeyCode,
        pressed: bool,
        repeat: bool,
        modifiers: ModifiersState,
        at: Instant,
    },
    /// The controls' input since the last.
    Ui(egui::RawInput),
    /// The window's new size in physical pixels.
    Resized(PhysicalSize<u32>),
    /// The window's new scale factor, as on a display of another density.
    ScaleFactorChanged(f64),
    Focused {
        focused: bool,
    },
    /// The window was asked to close.
    Close,
    /// An event of the map, from the runtime's thread.
    Map(MapEvent),
    /// What the controls show of the map's style, read on the runtime's
    /// thread. `loaded` is true when triggered by `MapEvent::StyleLoaded`.
    Style {
        info: view::StyleInfo,
        loaded: bool,
    },
    /// The cache database finished resetting, on the runtime's thread.
    CacheReset,
}

/// What the render thread asks of the main thread.
pub(crate) enum UserEvent {
    /// A surface for the window's Metal layer, from this instance, sent back
    /// on the channel. The window's thread makes it.
    #[cfg(target_os = "macos")]
    MakeSurface(wgpu::Instance, mpsc::Sender<Result<wgpu::Surface<'static>, String>>),
    /// A Metal layer in the window, sent back on the channel. The window's
    /// thread makes it.
    #[cfg(target_os = "macos")]
    MakeLayer(mpsc::Sender<Result<MetalLayer, String>>),
    SetTitle(String),
    RequestSize(LogicalSize<f64>),
    /// What egui asks of the window after a frame: its cursor, a link to open.
    Ui(Box<egui::PlatformOutput>),
    /// A gesture reached a notch: the trackpad ticks, where it can.
    Notch,
    /// The render thread has dropped the map and ended, with the error that
    /// stopped it opening the map, if one did.
    Ended(Option<String>),
}

/// The messages the render thread hasn't taken yet, and the wakes of the
/// map's render session. The render thread sleeps in [`wait`](Self::wait)
/// until one comes.
#[derive(Default)]
pub(crate) struct Inbox {
    state: Mutex<InboxState>,
    arrived: Condvar,
    /// The map has something new to draw.
    redraw: AtomicBool,
    /// The display refreshed since the render thread asked its frame pacer.
    refreshed: AtomicBool,
}

#[derive(Default)]
struct InboxState {
    messages: Vec<Message>,
    /// A message or a wake came since the render thread last waited.
    woken: bool,
}

impl Inbox {
    pub(crate) fn send(&self, message: Message) {
        let mut state = self.state.lock();
        state.messages.push(message);
        state.woken = true;
        drop(state);
        self.arrived.notify_one();
    }

    /// Wakes the render thread to service its render session.
    pub(crate) fn wake(&self) {
        self.state.lock().woken = true;
        self.arrived.notify_one();
    }

    /// Wakes the render thread to draw the map's latest update.
    pub(crate) fn wake_for_redraw(&self) {
        self.redraw.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Whether the map has had something new to draw since the last call.
    pub(crate) fn take_redraw(&self) -> bool {
        self.redraw.swap(false, Ordering::SeqCst)
    }

    /// Wakes the render thread at a display refresh, to draw a frame.
    pub(crate) fn wake_for_refresh(&self) {
        self.refreshed.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Whether the display refreshed since the last call.
    pub(crate) fn take_refresh(&self) -> bool {
        self.refreshed.swap(false, Ordering::SeqCst)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.state.lock().messages.is_empty()
    }

    pub(crate) fn take(&self) -> Vec<Message> {
        std::mem::take(&mut self.state.lock().messages)
    }

    /// Sleeps until a message or a wake comes, or until `deadline`. A
    /// message or wake that came since the last wait ends it at once; the
    /// messages that wait don't.
    pub(crate) fn wait(&self, deadline: Option<Instant>) {
        let mut state = self.state.lock();
        while !state.woken {
            match deadline {
                Some(deadline) => {
                    if self.arrived.wait_until(&mut state, deadline).timed_out() {
                        break;
                    }
                }
                None => self.arrived.wait(&mut state),
            }
        }
        state.woken = false;
    }
}

/// Opens the window and runs the app in it until it closes. Fails if the
/// app couldn't open the map.
pub(crate) fn run(args: Args, settings: Settings) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let mut host = Host { start: Some((args, settings)), proxy: event_loop.create_proxy(), open: None, failure: None };
    event_loop.run_app(&mut host).map_err(|e| e.to_string())?;
    host.failure.map_or(Ok(()), Err)
}

struct Host {
    /// What the render thread starts with, until the window opens.
    start: Option<(Args, Settings)>,
    proxy: EventLoopProxy<UserEvent>,
    open: Option<Open>,
    /// Why the app couldn't open the map, once the render thread says.
    failure: Option<String>,
}

/// The open window, on the main thread.
struct Open {
    window: Arc<Window>,
    inbox: Arc<Inbox>,
    egui: egui_winit::State,
    /// The map's pixel ratio: physical pixels per logical pixel.
    pixel_ratio: f64,
    /// Where the mouse is, in the map's logical pixels and in egui's points.
    cursor: ScreenCoordinate,
    cursor_points: egui::Pos2,
    /// The mouse buttons and touches that went down on the map.
    buttons: BTreeSet<MouseButton>,
    touches: BTreeSet<u32>,
    modifiers: ModifiersState,
    /// Whether the trackpad's pinch and rotation streams are under way:
    /// macOS phases each apart, and the map gets one pinch of both.
    pinching: bool,
    rotating: bool,
    /// The window's `CAMetalLayer`, made when a backend first asks for it.
    #[cfg(target_os = "macos")]
    metal_layer: Option<MetalLayer>,
}

impl Open {
    /// The window's `CAMetalLayer`, which both backends present to.
    #[cfg(target_os = "macos")]
    fn metal_layer(&mut self) -> Result<MetalLayer, String> {
        if let Some(layer) = &self.metal_layer {
            return Ok(layer.clone());
        }
        let layer = MetalLayer::new(&self.window)?;
        self.metal_layer = Some(layer.clone());
        Ok(layer)
    }

    fn logical(&self, position: PhysicalPosition<f64>) -> ScreenCoordinate {
        ScreenCoordinate::new(position.x / self.pixel_ratio, position.y / self.pixel_ratio)
    }

    /// Whether `position` is over one of the controls.
    fn over_controls(&self, position: PhysicalPosition<f64>) -> bool {
        let scale = self.window.scale_factor();
        crate::ui::over_controls(self.egui.egui_ctx(), (position.x / scale) as f32, (position.y / scale) as f32)
    }

    /// The controls' input so far, for the render thread.
    fn send_ui_input(&mut self, init: bool) {
        let mut input = self.egui.take_egui_input(&self.window);
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        egui_winit::update_viewport_info(viewport, self.egui.egui_ctx(), &self.window, init);
        self.inbox.send(Message::Ui(input));
    }

    fn input(&self, input: Input) {
        self.inbox.send(Message::Input { input, at: Instant::now() });
    }

    /// One of the trackpad's two streams, as part of a pinch of both: the
    /// first to begin begins the pinch, and the last to end ends it.
    fn trackpad(&mut self, rotating: bool, scale: f64, rotation: f64, phase: TouchPhase) {
        let active = self.pinching || self.rotating;
        // A pinch that begins over the controls isn't the map's.
        let over_controls = egui_over(self.egui.egui_ctx(), self.cursor_points);
        let stream = if rotating { &mut self.rotating } else { &mut self.pinching };
        let phase = match phase {
            TouchPhase::Started if over_controls && !active => return,
            TouchPhase::Started => {
                *stream = true;
                if active {
                    return;
                }
                PinchPhase::Begin
            }
            TouchPhase::Moved if *stream => PinchPhase::Update,
            TouchPhase::Ended | TouchPhase::Cancelled if *stream => {
                *stream = false;
                if self.pinching || self.rotating {
                    return;
                }
                if phase == TouchPhase::Ended { PinchPhase::End } else { PinchPhase::Cancel }
            }
            _ => return,
        };
        self.input(Input::Pinch { center: self.cursor, scale, rotation, phase });
    }
}

fn egui_over(ctx: &egui::Context, at: egui::Pos2) -> bool {
    crate::ui::over_controls(ctx, at.x, at.y)
}

/// What held keys do: Ctrl rotates and tilts on a drag, and Shift zooms to a
/// box on a drag and out on a double click.
fn key_modes(state: ModifiersState) -> Modes {
    Modes { rotate: state.control_key(), box_zoom: state.shift_key(), zoom_out: state.shift_key() }
}

/// The recognizer's pointer id for a touch: the mouse is 0.
fn touch_id(id: u64) -> u32 {
    (id % u64::from(u32::MAX)) as u32 + 1
}

impl ApplicationHandler<UserEvent> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some((args, settings)) = self.start.take() else {
            return;
        };
        let mut attributes = Window::default_attributes().with_title("mln-rs").with_inner_size(WINDOW_SIZE);
        if args.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        // A benchmark's frames keep the size the paths were made for.
        if args.benchmark.is_some() {
            attributes = attributes.with_resizable(false);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                eprintln!("failed to initialize window: {e}");
                event_loop.exit();
                return;
            }
        };
        let pixel_ratio = window.scale_factor();
        // A frame per refresh of the display the window opens on.
        let refresh_hz = window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .map_or(60.0, |mhz| f64::from(mhz) / 1000.0);
        let inbox = Arc::new(Inbox::default());
        let ui = egui::Context::default();
        // The backends' logos are SVGs (ui.rs).
        egui_extras::install_image_loaders(&ui);
        let egui = egui_winit::State::new(
            ui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        // Benchmark frames don't wait for the display.
        let frame_pacer = args.benchmark.is_none().then(|| frame_pacer(&window));
        let launch = Launch {
            frame_pacer,
            #[cfg(all(not(target_os = "macos"), not(windows)))]
            window: Arc::clone(&window),
            #[cfg(windows)]
            window: Arc::new(crate::win32_window::SurfaceWindow::new(Arc::clone(&window))),
            ui,
            args,
            settings,
            pixel_ratio,
            size: window.inner_size(),
            frame_interval: Duration::from_secs_f64(1.0 / refresh_hz.max(1.0)),
            start: Instant::now(),
            inbox: Arc::clone(&inbox),
            proxy: self.proxy.clone(),
        };
        let mut open = Open {
            window,
            inbox,
            egui,
            pixel_ratio,
            cursor: ScreenCoordinate::default(),
            cursor_points: egui::Pos2::ZERO,
            buttons: BTreeSet::new(),
            touches: BTreeSet::new(),
            modifiers: ModifiersState::default(),
            pinching: false,
            rotating: false,
            #[cfg(target_os = "macos")]
            metal_layer: None,
        };
        open.send_ui_input(true);
        self.open = Some(open);

        let proxy = self.proxy.clone();
        let spawned = mln_engine::exec::thread::builder("render").spawn(move || {
            // Sent even if the render thread panics, so the window closes.
            struct Ended(EventLoopProxy<UserEvent>, Option<String>);
            impl Drop for Ended {
                fn drop(&mut self) {
                    let _ = self.0.send_event(UserEvent::Ended(self.1.take()));
                }
            }
            let mut ended = Ended(proxy, None);
            ended.1 = view::run(launch).err();
        });
        if let Err(e) = spawned {
            self.failure = Some(format!("failed to start the render thread: {e}"));
            event_loop.exit();
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let response = open.egui.on_window_event(&open.window, &event);
        if response.repaint {
            open.send_ui_input(false);
        }
        match event {
            // The render thread drops the map, then says it has ended.
            WindowEvent::CloseRequested => open.inbox.send(Message::Close),
            WindowEvent::Resized(size) => open.inbox.send(Message::Resized(size)),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                open.pixel_ratio = scale_factor;
                open.inbox.send(Message::ScaleFactorChanged(scale_factor));
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    // Drags and keys the window can't see the end of end here.
                    if !std::mem::take(&mut open.buttons).is_empty() {
                        open.input(Input::PointerCancel { id: 0 });
                    }
                    for id in std::mem::take(&mut open.touches) {
                        open.input(Input::PointerCancel { id });
                    }
                    open.modifiers = ModifiersState::default();
                    open.input(Input::Modes(Modes::default()));
                }
                open.inbox.send(Message::Focused { focused });
            }
            WindowEvent::ModifiersChanged(state) => {
                open.modifiers = state.state();
                open.input(Input::Modes(key_modes(open.modifiers)));
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } if !response.consumed => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    open.inbox.send(Message::Key {
                        code,
                        pressed: event.state == ElementState::Pressed,
                        repeat: event.repeat,
                        modifiers: open.modifiers,
                        at: Instant::now(),
                    });
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                open.cursor = open.logical(position);
                let scale = open.window.scale_factor();
                open.cursor_points = egui::pos2((position.x / scale) as f32, (position.y / scale) as f32);
                if !open.buttons.is_empty() || !open.over_controls(position) {
                    open.input(Input::PointerMove { id: 0, position: open.cursor });
                }
            }
            // The left button drags, the right one rotates and tilts, and the
            // others mean nothing to the map.
            WindowEvent::MouseInput { state, button, .. } => {
                let modes = match button {
                    MouseButton::Left => Modes::default(),
                    MouseButton::Right => Modes { rotate: true, ..Modes::default() },
                    _ => return,
                };
                let position = open.cursor;
                match state {
                    ElementState::Pressed if !egui_over(open.egui.egui_ctx(), open.cursor_points) => {
                        open.buttons.insert(button);
                        open.input(Input::PointerDown { id: 0, kind: PointerKind::Mouse, position, modes });
                    }
                    ElementState::Released if open.buttons.remove(&button) => {
                        open.input(Input::PointerUp { id: 0, position });
                    }
                    _ => {}
                }
            }
            // winit's positive y scrolls up, which zooms in. Line deltas are a
            // wheel's detents, a third of a level each, eased; pixel deltas a
            // trackpad's or a precise wheel's scrolling, 1/300 level a pixel.
            WindowEvent::MouseWheel { delta, .. } => {
                let (discrete, zoom) = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (true, f64::from(y) / 3.0),
                    MouseScrollDelta::PixelDelta(pixels) => (false, pixels.y / open.pixel_ratio / 300.0),
                };
                if zoom != 0.0 && !egui_over(open.egui.egui_ctx(), open.cursor_points) {
                    let pan = ScreenCoordinate::default();
                    open.input(Input::Delta { position: open.cursor, discrete, pan, zoom, bearing: 0.0, pitch: 0.0 });
                }
            }
            WindowEvent::Touch(touch) => {
                let id = touch_id(touch.id);
                let position = open.logical(touch.location);
                let input = match touch.phase {
                    TouchPhase::Started if !open.over_controls(touch.location) => {
                        open.touches.insert(id);
                        Input::PointerDown { id, kind: PointerKind::Touch, position, modes: Modes::default() }
                    }
                    TouchPhase::Moved if open.touches.contains(&id) => Input::PointerMove { id, position },
                    TouchPhase::Ended if open.touches.remove(&id) => Input::PointerUp { id, position },
                    TouchPhase::Cancelled if open.touches.remove(&id) => Input::PointerCancel { id },
                    _ => return,
                };
                open.input(input);
            }
            // macOS's trackpad magnify and rotate (winit has them nowhere
            // else). winit's rotation turns counterclockwise.
            WindowEvent::PinchGesture { delta, phase, .. } => open.trackpad(false, delta.exp(), 0.0, phase),
            WindowEvent::RotationGesture { delta, phase, .. } => {
                open.trackpad(true, 1.0, -f64::from(delta), phase);
            }
            // The render thread draws when it has something new.
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match event {
            #[cfg(target_os = "macos")]
            UserEvent::MakeSurface(instance, reply) => {
                let _ = reply.send(open.metal_layer().and_then(|layer| layer.create_surface(&instance)));
            }
            #[cfg(target_os = "macos")]
            UserEvent::MakeLayer(reply) => {
                let _ = reply.send(open.metal_layer());
            }
            UserEvent::SetTitle(title) => open.window.set_title(&title),
            // A Force Touch trackpad's alignment tick, as AppKit plays when a
            // dragged item snaps into place; elsewhere nothing to play.
            #[cfg(target_os = "macos")]
            UserEvent::Notch => {
                use objc2_app_kit::{
                    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
                    NSHapticFeedbackPerformer,
                };
                NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                    NSHapticFeedbackPattern::Alignment,
                    NSHapticFeedbackPerformanceTime::Now,
                );
            }
            #[cfg(not(target_os = "macos"))]
            UserEvent::Notch => {}
            UserEvent::Ui(output) => open.egui.handle_platform_output(&open.window, *output),
            UserEvent::RequestSize(size) => {
                // A size that is applied at once (Wayland) comes back here
                // and not as a `Resized` event.
                if let Some(size) = open.window.request_inner_size(size) {
                    open.inbox.send(Message::Resized(size));
                }
            }
            UserEvent::Ended(failure) => {
                self.failure = failure;
                event_loop.exit();
            }
        }
    }
}
