#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mln_runtime::{Completion, Error, Map, MapOptions, Runtime, RuntimeEvent, RuntimeOptions};
use mln_util::size::Size;

pub const TIMEOUT: Duration = Duration::from_secs(10);

pub const EMPTY_STYLE: &str = r#"{"version":8,"sources":{},"layers":[]}"#;

pub fn runtime() -> TestRuntime {
    TestRuntime(Runtime::new(&RuntimeOptions::default()).unwrap())
}

/// A runtime that shuts down fully when the test ends: once every map, and
/// every render session that holds one, has let go, which a dropped handle
/// only starts. A session still freeing its GPU objects when the test binary
/// exits can call into a Vulkan layer or driver that the exit unloaded.
pub struct TestRuntime(Runtime);

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

pub fn map_options() -> MapOptions {
    MapOptions { map: mln_map::MapOptions { size: Size::new(256, 256), ..Default::default() }, ..Default::default() }
}

/// A map that the runtime has built.
pub fn map(runtime: &Runtime) -> Map {
    let (map, created) = runtime.create_map(map_options()).unwrap();
    created.wait().unwrap();
    map
}

/// The result of a command that was sent.
pub fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> T {
    completion.unwrap().wait().unwrap()
}

/// The error a command that was sent completes with.
pub fn wait_err<T: Send + 'static + std::fmt::Debug>(completion: Result<Completion<T>, Error>) -> Error {
    completion.unwrap().wait().unwrap_err()
}

/// Waits until `done` holds, polling.
pub fn eventually(mut done: impl FnMut() -> bool) -> bool {
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
pub struct EventLog {
    shared: Arc<(Mutex<Vec<RuntimeEvent>>, Condvar)>,
}

impl EventLog {
    /// Sets the runtime's handler to one that records into the log.
    pub fn install(runtime: &Runtime) -> EventLog {
        let log = EventLog::default();
        let recorder = log.clone();
        wait(runtime.set_event_handler(move |events| recorder.push(events)));
        log
    }

    pub fn push(&self, events: &[RuntimeEvent]) {
        let (events_so_far, changed) = &*self.shared;
        events_so_far.lock().unwrap().extend_from_slice(events);
        changed.notify_all();
    }

    pub fn events(&self) -> Vec<RuntimeEvent> {
        self.shared.0.lock().unwrap().clone()
    }

    /// Waits until the events hold an event that `wanted` accepts.
    pub fn wait_for(&self, mut wanted: impl FnMut(&RuntimeEvent) -> bool) -> Option<RuntimeEvent> {
        let (events, changed) = &*self.shared;
        let deadline = Instant::now() + TIMEOUT;
        let mut events = events.lock().unwrap();
        loop {
            if let Some(event) = events.iter().find(|event| wanted(event)) {
                return Some(event.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            events = changed.wait_timeout(events, deadline - now).unwrap().0;
        }
    }
}

/// A directory that is deleted when it drops.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mln-runtime-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
