//! The basemap on a map of an `mln_runtime` runtime, drawn by an offscreen
//! render session, as maplibre-native-ffi's benchmarks drive one through
//! its C API: for the cases that measure the runtime's own work, its
//! commands, rather than a frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mln_map::{CameraOptions, CameraUpdate, MapMode, RenderMode};
use mln_runtime::{
    AttachOptions, DemandMode, EventMask, EventType, FrameDemand, FrameDisposition, FrameResult, Map, MapOptions,
    RenderSession, RenderTarget, Runtime, RuntimeOptions,
};
use mln_util::geo::LatLng;
use parking_lot::{Condvar, Mutex};

use crate::Context;
use crate::scene::{BASEMAP_SIZE, BASEMAP_ZOOM, basemap_center, basemap_style};

/// How long the scene waits for a frame, or for the map to settle.
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

pub(crate) struct RuntimeScene {
    runtime: Runtime,
    map: Map,
    session: RenderSession,
    wakes: Arc<Wakes>,
    token: u64,
}

impl RuntimeScene {
    /// The basemap at its center on a new runtime, whose handler receives
    /// the map's events, with an offscreen session attached and the map
    /// settled.
    pub(crate) fn basemap(cx: &Context) -> RuntimeScene {
        let runtime = Runtime::new(&RuntimeOptions::default()).expect("a runtime");
        let events = Arc::new(AtomicU64::new(0));
        runtime
            .set_event_handler(move |batch| {
                events.fetch_add(batch.len() as u64, Ordering::Relaxed);
            })
            .expect("an event handler")
            .wait()
            .expect("the event handler");
        let options = MapOptions {
            map: mln_map::MapOptions {
                map_mode: MapMode::Continuous,
                size: BASEMAP_SIZE,
                pixel_ratio: 1.0,
                ..Default::default()
            },
            event_mask: EVENTS.into_iter().fold(EventMask::NONE, EventMask::with),
            ..Default::default()
        };
        let (map, created) = runtime.create_map(options).expect("a map");
        created.wait().expect("the map");
        map.update_camera(jump(basemap_center())).expect("a camera").wait().expect("the camera");
        map.set_style_json(basemap_style(&cx.fixtures)).expect("a style").wait().expect("the style");
        let wakes = Arc::new(Wakes::default());
        let wake = Arc::clone(&wakes);
        let attach = AttachOptions { frame_wake: Some(Arc::new(move || wake.notify())), ..Default::default() };
        let (session, attached) =
            map.attach(RenderTarget::Offscreen { size: BASEMAP_SIZE, device: None }, attach).expect("a session");
        attached.wait().expect("the session");
        let mut scene = RuntimeScene { runtime, map, session, wakes, token: 0 };
        scene.settle();
        scene
    }

    pub(crate) fn map(&self) -> &Map {
        &self.map
    }

    /// Draws a frame of `mode` and waits for its result.
    fn render(&mut self, mode: DemandMode) -> FrameResult {
        self.token += 1;
        let token = self.token;
        let deadline = Instant::now() + WAIT_LIMIT;
        self.session.demand_frame(FrameDemand::new(mode, token)).expect("a frame demand");
        loop {
            // Before the drain, so that a wake for a result that the drain
            // misses ends the wait.
            let seen = self.wakes.seen();
            let results = self.session.drain_frame_results().expect("the frame results");
            if let Some(result) = results.into_iter().find(|result| result.token == Some(token)) {
                return result;
            }
            assert!(Instant::now() < deadline, "no frame after {WAIT_LIMIT:?}");
            self.wakes.wait(seen, deadline);
        }
    }

    /// Draws frames until one shows the map settled: complete, with no
    /// transition left, and drawing its latest update.
    pub(crate) fn settle(&mut self) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut mode = DemandMode::IfChanged;
        loop {
            let result = self.render(mode);
            if result.disposition == FrameDisposition::Rendered {
                let snapshot = self.map.snapshot().expect("a snapshot");
                if result.mode == RenderMode::Full
                    && !result.needs_repaint
                    && !snapshot.transition_in_progress
                    && result.map_update_generation >= snapshot.update_generation
                {
                    return;
                }
                mode = if result.needs_repaint { DemandMode::Force } else { DemandMode::WaitForUpdate };
            } else {
                mode = DemandMode::Force;
            }
            assert!(Instant::now() < deadline, "the map isn't settled after {WAIT_LIMIT:?}");
        }
    }
}

impl Drop for RuntimeScene {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.session.release().expect("the session's release").wait().expect("the session released");
        self.map.release().expect("the map's release").wait().expect("the map released");
        self.runtime.release().expect("the runtime's release").wait().expect("the runtime released");
    }
}

/// A camera jump to `center` at the basemap's zoom.
pub(crate) fn jump(center: LatLng) -> CameraUpdate {
    CameraUpdate {
        camera: CameraOptions { center: Some(center), zoom: Some(BASEMAP_ZOOM), ..Default::default() },
        ..Default::default()
    }
}
