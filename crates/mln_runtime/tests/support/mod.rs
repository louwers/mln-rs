#![expect(dead_code, reason = "each test binary uses some of the shared helpers")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mln_engine::map::MapMode;
use mln_engine::storage::Response;
use mln_runtime::{
    AttachOptions, Completion, Error, Map, MapOptions, RenderSession, RenderTarget, ResourceRequest, Runtime,
    RuntimeEvent, RuntimeOptions,
};
use mln_style::size::Size;
use parking_lot::{Condvar, Mutex};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const EMPTY_STYLE: &str = r#"{"version":8,"sources":{},"layers":[]}"#;

pub(crate) fn runtime() -> TestRuntime {
    TestRuntime(Runtime::new(&RuntimeOptions::default()).unwrap())
}

/// A runtime that shuts down fully when the test ends: once every map, and
/// every render session that holds one, has let go, which a dropped handle
/// only starts. A session still freeing its GPU objects when the test binary
/// exits can call into a Vulkan layer or driver that the exit unloaded.
pub(crate) struct TestRuntime(pub(crate) Runtime);

impl std::ops::Deref for TestRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.0
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.0.release() {
                Ok(released) => {
                    released.wait().expect("the runtime shuts down");
                    return;
                }
                // Refused while a map, or a session holding one, hasn't
                // let go yet.
                Err(error) if error.message.contains("maps first") && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Released by the test already, or a map outlived the
                // deadline, which the test that leaked it shows.
                Err(_) => return,
            }
        }
    }
}

pub(crate) fn map_options() -> MapOptions {
    MapOptions {
        map: mln_engine::map::MapOptions { size: Size::new(256, 256), ..Default::default() },
        ..Default::default()
    }
}

/// A map that the runtime has built.
pub(crate) fn map(runtime: &Runtime) -> Map {
    let (map, created) = runtime.create_map(map_options()).unwrap();
    created.wait().unwrap();
    map
}

/// A style centered on 10° N, 20° E at zoom 12 and pitch 60, over the
/// terrain of [`serve_level_dem`]'s tiles.
pub(crate) const LEVEL_TERRAIN_STYLE: &str = r##"{"version":8,"center":[20,10],"zoom":12,"pitch":60,"terrain":{"source":"dem"},"sources":{"dem":{"type":"raster-dem","tiles":["custom://dem/{z}/{x}/{y}.png"],"maxzoom":0}},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}}]}"##;

/// Answers every resource request of `runtime` with a DEM tile of ground
/// 1000 m high everywhere, in the Mapbox encoding:
/// (1 · 65536 + 173 · 256 + 176) · 0.1 − 10000.
pub(crate) fn serve_level_dem(runtime: &Runtime) {
    let mut dem = mln_style::bitmap::PremultipliedImage::new(Size::new(4, 4));
    for pixel in dem.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 173, 176, 255]);
    }
    let dem = Arc::new(mln_image::encode_png(&dem));
    wait(runtime.set_resource_provider(move |request: ResourceRequest| {
        let _ = request.respond(Response { data: Some(Arc::clone(&dem)), ..Response::default() });
    }));
}

/// A 256 by 256 still map of [`LEVEL_TERRAIN_STYLE`] on `runtime`, which
/// [`serve_level_dem`] serves, whose center the terrain doesn't clamp, so
/// it stays at sea level, and the offscreen session that draws its stills.
/// It has drawn none yet.
pub(crate) fn level_terrain_map(runtime: &Runtime) -> (Map, RenderSession) {
    let options = MapOptions {
        map: mln_engine::map::MapOptions {
            size: Size::new(256, 256),
            map_mode: MapMode::Static,
            center_clamped_to_ground: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    result_of(created).unwrap();
    wait(map.set_style_json(LEVEL_TERRAIN_STYLE));
    let target = RenderTarget::Offscreen { size: Size::new(256, 256), device: None };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    result_of(attached).unwrap();
    (map, session)
}

/// How long a test waits for a completion before it fails, which is far
/// longer than any command takes.
pub(crate) const COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// Waits for the result of `completion`. A result that doesn't arrive within
/// [`COMPLETION_TIMEOUT`] fails the test at the caller's line, instead of
/// hanging it.
#[track_caller]
pub(crate) fn result_of<T: Send + 'static>(completion: Completion<T>) -> Result<T, Error> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    completion.on_complete(move |result| {
        let _ = sender.send(result);
    });
    let Ok(result) = receiver.recv_timeout(COMPLETION_TIMEOUT) else {
        panic!("the completion waited for here didn't complete within {COMPLETION_TIMEOUT:?}");
    };
    result
}

/// The result of a command that was sent.
#[track_caller]
pub(crate) fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> T {
    result_of(completion.unwrap()).unwrap()
}

/// The error a command that was sent completes with.
#[track_caller]
pub(crate) fn wait_err<T: Send + 'static + std::fmt::Debug>(completion: Result<Completion<T>, Error>) -> Error {
    result_of(completion.unwrap()).unwrap_err()
}

/// Waits until `done` holds, polling.
pub(crate) fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    done()
}

/// The events that a runtime's handler received.
#[derive(Clone, Default)]
pub(crate) struct EventLog {
    shared: Arc<(Mutex<Vec<RuntimeEvent>>, Condvar)>,
}

impl EventLog {
    /// Sets the runtime's handler to one that records into the log.
    pub(crate) fn install(runtime: &Runtime) -> EventLog {
        let log = EventLog::default();
        let recorder = log.clone();
        wait(runtime.set_event_handler(move |events| recorder.push(events)));
        log
    }

    pub(crate) fn push(&self, events: &[RuntimeEvent]) {
        let (events_so_far, changed) = &*self.shared;
        events_so_far.lock().extend_from_slice(events);
        changed.notify_all();
    }

    pub(crate) fn events(&self) -> Vec<RuntimeEvent> {
        self.shared.0.lock().clone()
    }

    /// Waits until the events hold an event that `wanted` accepts, or gives
    /// `None` after [`COMPLETION_TIMEOUT`]. An event can wait on what a
    /// command waits on, such as the network, so it gets as long.
    pub(crate) fn wait_for(&self, mut wanted: impl FnMut(&RuntimeEvent) -> bool) -> Option<RuntimeEvent> {
        let (events, changed) = &*self.shared;
        let deadline = Instant::now() + COMPLETION_TIMEOUT;
        let mut events = events.lock();
        loop {
            if let Some(event) = events.iter().find(|event| wanted(event)) {
                return Some(event.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            changed.wait_for(&mut events, deadline - now);
        }
    }
}

/// A directory that is deleted when it drops.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mln-runtime-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
