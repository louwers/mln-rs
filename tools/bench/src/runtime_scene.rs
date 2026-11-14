//! The basemap on maps of an `mln_runtime` runtime, drawn by offscreen
//! render sessions, as maplibre-native-ffi's benchmarks drive them through
//! its C API: for the cases that measure the runtime's own work, its
//! commands and its maps, rather than a frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use feature::Value;
use mln_map::{CameraOptions, CameraUpdate, MapMode, RenderMode};
use mln_runtime::{
    AttachOptions, DemandMode, Device, EventMask, EventType, FrameDemand, FrameDisposition, FrameResult, Map,
    MapOptions, RenderSession, RenderTarget, Runtime, RuntimeOptions, SourceQueryOptions,
};
use mln_util::feature::Feature;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use parking_lot::{Condvar, Mutex};

use crate::Context;
use crate::scene::{BASEMAP_SIZE, BASEMAP_ZOOM, REFRESH, basemap_center, basemap_style};

/// How long a map waits for a frame, or to settle.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

/// The events that maplibre-native-ffi's benchmarks ask a map for: those a
/// host's map view typically follows.
const EVENTS: [EventType; 10] = [
    EventType::MapCameraWillChange,
    EventType::MapCameraDidChange,
    EventType::MapStyleLoaded,
    EventType::MapLoadingStarted,
    EventType::MapLoadingFinished,
    EventType::MapLoadingFailed,
    EventType::MapIdle,
    EventType::MapRenderError,
    EventType::MapRenderFrameFinished,
    EventType::MapStyleImageMissing,
];

/// Counts the session's frame wakes, and wakes the thread that waits for
/// one.
#[derive(Default)]
struct Wakes {
    count: Mutex<u64>,
    changed: Condvar,
}

impl Wakes {
    fn notify(&self) {
        *self.count.lock() += 1;
        self.changed.notify_all();
    }

    fn seen(&self) -> u64 {
        *self.count.lock()
    }

    /// Waits for a wake after `seen`, until `deadline` at the latest.
    fn wait(&self, seen: u64, deadline: Instant) {
        let mut count = self.count.lock();
        while *count == seen && !self.changed.wait_until(&mut count, deadline).timed_out() {}
    }
}

/// A runtime started with `options`, whose event handler, its first
/// command, has been set: a host's runtime once it's ready. It's released,
/// and waited for, when dropped; its maps must be dropped before.
pub(crate) struct RuntimeHandle(Runtime);

impl RuntimeHandle {
    pub(crate) fn new(options: &RuntimeOptions) -> RuntimeHandle {
        let runtime = Runtime::new(options).expect("a runtime");
        let events = Arc::new(AtomicU64::new(0));
        runtime
            .set_event_handler(move |batch| {
                events.fetch_add(batch.len() as u64, Ordering::Relaxed);
            })
            .expect("an event handler")
            .wait()
            .expect("the event handler");
        RuntimeHandle(runtime)
    }

    pub(crate) fn runtime(&self) -> &Runtime {
        &self.0
    }

    /// A new continuous map of `size`, that sends the runtime's handler the
    /// events of [`EVENTS`].
    pub(crate) fn create_map(&self, size: Size) -> Map {
        let options = MapOptions {
            map: mln_map::MapOptions { map_mode: MapMode::Continuous, size, pixel_ratio: 1.0, ..Default::default() },
            event_mask: EVENTS.into_iter().fold(EventMask::NONE, EventMask::with),
            ..Default::default()
        };
        let (map, created) = self.0.create_map(options).expect("a map");
        created.wait().expect("the map");
        map
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.0.release().expect("the runtime's release").wait().expect("the runtime released");
    }
}

/// A map of a runtime with an offscreen session attached. The session and
/// the map are released, and waited for, when it's dropped.
pub(crate) struct RuntimeMap {
    map: Map,
    session: RenderSession,
    wakes: Arc<Wakes>,
    token: u64,
    /// Each frame's time since the last [`take_frame_times`](Self::take_frame_times),
    /// from asking for it until its result arrives.
    times: Vec<Duration>,
}

/// Where a [`RuntimeMap`] looks, and what at.
pub(crate) struct View {
    pub(crate) size: Size,
    pub(crate) center: LatLng,
    pub(crate) zoom: f64,
    pub(crate) style: String,
}

impl RuntimeMap {
    /// The basemap at its center on a new map of `runtime`, its style
    /// loaded and an offscreen session attached on `device`, or on a device
    /// of its own; nothing is drawn yet.
    pub(crate) fn basemap(runtime: &RuntimeHandle, cx: &Context, device: Option<Device>) -> RuntimeMap {
        let view = View {
            size: BASEMAP_SIZE,
            center: basemap_center(),
            zoom: BASEMAP_ZOOM,
            style: basemap_style(&cx.fixtures),
        };
        RuntimeMap::new(runtime, view, device)
    }

    /// A new map of `runtime` showing `view`, with an offscreen session
    /// attached on `device`, or on a device of its own; nothing is drawn
    /// yet.
    pub(crate) fn new(runtime: &RuntimeHandle, view: View, device: Option<Device>) -> RuntimeMap {
        let View { size, center, zoom, style } = view;
        let map = runtime.create_map(size);
        map.update_camera(jump(center, zoom)).expect("a camera").wait().expect("the camera");
        map.set_style_json(style).expect("a style").wait().expect("the style");
        let wakes = Arc::new(Wakes::default());
        let wake = Arc::clone(&wakes);
        let attach = AttachOptions { frame_wake: Some(Arc::new(move || wake.notify())), ..Default::default() };
        let (session, attached) = map.attach(RenderTarget::Offscreen { size, device }, attach).expect("a session");
        attached.wait().expect("the session");
        RuntimeMap { map, session, wakes, token: 0, times: Vec::new() }
    }

    pub(crate) fn map(&self) -> &Map {
        &self.map
    }

    /// Draws a frame of `mode` and waits for its result.
    fn render(&mut self, mode: DemandMode) -> FrameResult {
        self.token += 1;
        let token = self.token;
        let demanded = Instant::now();
        let deadline = demanded + WAIT_LIMIT;
        self.session.demand_frame(FrameDemand::new(mode, token)).expect("a frame demand");
        loop {
            // Before the drain, so that a wake for a result that the drain
            // misses ends the wait.
            let seen = self.wakes.seen();
            let results = self.session.drain_frame_results().expect("the frame results");
            if let Some(result) = results.into_iter().find(|result| result.token == Some(token)) {
                self.times.push(demanded.elapsed());
                return result;
            }
            assert!(Instant::now() < deadline, "no frame after {WAIT_LIMIT:?}");
            self.wakes.wait(seen, deadline);
        }
    }

    /// Draws frames on a 60 Hz display's clock, each at the first refresh
    /// after the last one's result, until one that draws the map's latest
    /// update is complete and `settled` holds for it; returns the frames
    /// drawn. Panics at `deadline`.
    fn render_until(&mut self, deadline: Instant, settled: impl Fn(&FrameResult, bool) -> bool) -> usize {
        let start = Instant::now();
        let mut mode = DemandMode::IfChanged;
        let mut frames = 0;
        let mut refresh = start;
        loop {
            std::thread::sleep(refresh.saturating_duration_since(Instant::now()));
            let result = self.render(mode);
            // The refresh after now: a frame that took longer than one
            // skips the refreshes it missed rather than catching up.
            let elapsed = start.elapsed().as_nanos() / REFRESH.as_nanos();
            refresh = start + REFRESH * (u32::try_from(elapsed).expect("a refresh count") + 1);
            if result.disposition == FrameDisposition::Rendered {
                frames += 1;
                let snapshot = self.map.snapshot().expect("a snapshot");
                if result.mode == RenderMode::Full
                    && result.map_update_generation >= snapshot.update_generation
                    && settled(&result, snapshot.transition_in_progress)
                {
                    return frames;
                }
                mode = if result.needs_repaint { DemandMode::Force } else { DemandMode::WaitForUpdate };
            } else {
                mode = DemandMode::Force;
            }
            assert!(Instant::now() < deadline, "the map isn't complete after {WAIT_LIMIT:?}");
        }
    }

    /// Draws frames until one shows the map's latest update complete;
    /// returns the frames drawn.
    pub(crate) fn present_until_complete(&mut self) -> usize {
        self.render_until(Instant::now() + WAIT_LIMIT, |_, _| true)
    }

    /// Draws frames until one shows the map settled: complete, with no
    /// transition left, and drawing its latest update; returns the frames
    /// drawn.
    pub(crate) fn settle(&mut self) -> usize {
        self.settle_by(Instant::now() + WAIT_LIMIT)
    }

    fn settle_by(&mut self, deadline: Instant) -> usize {
        self.render_until(deadline, |result, transition| !result.needs_repaint && !transition)
    }

    /// Draws frames until one shows the map settled, as
    /// [`settle`](Self::settle), and every feature of `source` has `value`
    /// for `key`; returns the frames drawn.
    pub(crate) fn settle_until_source_shows(&mut self, source: &str, key: &str, value: u64) -> usize {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut frames = self.settle_by(deadline);
        while !self.source_shows(source, key, value) {
            assert!(Instant::now() < deadline, "{source} doesn't show {key} {value} after {WAIT_LIMIT:?}");
            frames += self.settle_by(deadline);
        }
        frames
    }

    /// Whether the session's last frame had features of `source` and every
    /// one has `value` for `key`.
    fn source_shows(&self, source: &str, key: &str, value: u64) -> bool {
        let features = self
            .session
            .query_source_features(source, SourceQueryOptions::new(None, None))
            .expect("a source query")
            .wait()
            .expect("the source's features");
        features_show(&features, key, value)
    }

    /// The frame times since the last call, as `times` describes.
    pub(crate) fn take_frame_times(&mut self) -> Vec<Duration> {
        std::mem::take(&mut self.times)
    }

    /// The last frame the session drew.
    pub(crate) fn read_frame(&self) -> PremultipliedImage {
        self.session.read_texture().expect("a readback").wait().expect("the frame")
    }
}

impl Drop for RuntimeMap {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.session.release().expect("the session's release").wait().expect("the session released");
        self.map.release().expect("the map's release").wait().expect("the map released");
    }
}

/// The basemap on the one map of a new runtime, settled.
pub(crate) struct RuntimeScene {
    // Dropped in this order: a runtime is released after its maps.
    map: RuntimeMap,
    _runtime: RuntimeHandle,
}

impl RuntimeScene {
    /// The basemap at its center on a new runtime with the default options,
    /// whose handler receives the map's events, with an offscreen session
    /// attached and the map settled.
    pub(crate) fn basemap(cx: &Context) -> RuntimeScene {
        let runtime = RuntimeHandle::new(&RuntimeOptions::default());
        let mut map = RuntimeMap::basemap(&runtime, cx, None);
        map.settle();
        RuntimeScene { map, _runtime: runtime }
    }

    pub(crate) fn map(&self) -> &Map {
        self.map.map()
    }

    /// See [`RuntimeMap::present_until_complete`].
    pub(crate) fn present_until_complete(&mut self) -> usize {
        self.map.present_until_complete()
    }

    /// See [`RuntimeMap::read_frame`].
    pub(crate) fn read_frame(&self) -> PremultipliedImage {
        self.map.read_frame()
    }
}

/// A camera jump to `center` at `zoom`.
pub(crate) fn jump(center: LatLng, zoom: f64) -> CameraUpdate {
    CameraUpdate {
        camera: CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() },
        ..Default::default()
    }
}

/// Whether there are `features` and every one has `value` for `key`.
fn features_show(features: &[Feature], key: &str, value: u64) -> bool {
    !features.is_empty()
        && features.iter().all(|feature| match feature.properties.get(key) {
            Some(Value::Uint(number)) => *number == value,
            Some(Value::Int(number)) => u64::try_from(*number) == Ok(value),
            _ => false,
        })
}
