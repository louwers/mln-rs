// Ported gtests (test/storage/*.test.cpp) and test helpers.

// Most helpers serve the SQLite tests.
#![cfg_attr(not(feature = "sqlite"), allow(dead_code))]

mod asset_file_source;
#[cfg(feature = "sqlite")]
mod database_file_source;
mod file_source_manager;
mod http_file_source;
mod local_file_source;
// The C++ MainResourceLoader tests assume its Database file source.
#[cfg(feature = "sqlite")]
mod main_resource_loader;
#[cfg(feature = "sqlite")]
mod mbtiles_file_source;
mod offline;
#[cfg(feature = "sqlite")]
mod offline_database;
mod online_file_source;
mod pmtiles_file_source;
#[cfg(feature = "sqlite")]
mod sqlite;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, mpsc};

use mln_actor::ActorRef;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use parking_lot::{Mutex, MutexGuard};

use crate::AsyncRequest;

/// A `std::unique_ptr<AsyncRequest>` that a request's own callback can reset.
pub(crate) type Req = Rc<RefCell<Option<Box<dyn AsyncRequest>>>>;

pub(crate) fn req() -> Req {
    Rc::new(RefCell::new(None))
}

/// Serializes the tests that use process-wide state (`NetworkStatus`, the
/// log observer): gtest runs one test at a time, the Rust harness runs them
/// in parallel.
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
}

/// Holds `actor`'s thread in a message, after the messages already sent to
/// it, until the returned guard drops. It returns once the thread is held, so
/// the messages sent meanwhile queue up and run in one go after the drop. Drop
/// the guard before the actor's `Thread`, whose drop waits for its queue.
pub(crate) fn pause<O: 'static>(actor: &ActorRef<O>) -> Paused {
    let (paused, is_paused) = mpsc::channel();
    let (resume, resumed) = mpsc::channel::<()>();
    actor.invoke(move |_| {
        let _ = paused.send(());
        // Returns once `Paused` drops its sender.
        let _ = resumed.recv();
    });
    is_paused.recv().expect("the thread holds");
    Paused { _resume: resume }
}

/// Resumes the thread that [`pause`] holds when it drops.
pub(crate) struct Paused {
    _resume: mpsc::Sender<()>,
}

/// A path of the C++ repository, in the copy of its files these tests read
/// (testdata/).
pub(crate) fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

/// A fresh path in a temporary directory of its own (C++ tests write next to
/// the fixtures in the source tree; the port never writes there). The
/// directory is unique to this process, so concurrent test runs never share
/// a file, and dropping the guard removes it with everything SQLite wrote
/// beside the path.
pub(crate) fn temp_path(name: &str) -> TempPath {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mln-storage-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("Cannot create {}: {e}", dir.display()));
    let path = dir.join(name).to_string_lossy().into_owned();
    TempPath { dir, path }
}

/// A path from [`temp_path`], usable as a `&str`.
pub(crate) struct TempPath {
    dir: PathBuf,
    path: String,
}

impl std::ops::Deref for TempPath {
    type Target = str;

    fn deref(&self) -> &str {
        &self.path
    }
}

impl std::fmt::Display for TempPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path)
    }
}

impl From<&TempPath> for String {
    fn from(path: &TempPath) -> Self {
        path.path.clone()
    }
}

impl AsRef<std::path::Path> for TempPath {
    fn as_ref(&self) -> &std::path::Path {
        self.path.as_ref()
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Message {
    pub(crate) severity: EventSeverity,
    pub(crate) event: Event,
    pub(crate) code: i64,
    pub(crate) msg: String,
}

struct Recorder(Arc<Mutex<Vec<(Message, bool)>>>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push((Message { severity, event, code, msg: msg.to_owned() }, false));
        true
    }
}

/// Records the calling thread's log messages for a test to check.
pub(crate) struct FixtureLog {
    messages: Arc<Mutex<Vec<(Message, bool)>>>,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(messages.clone()))));
        FixtureLog { messages }
    }

    /// `count(message, substring)`: marks the matches checked.
    pub(crate) fn count(&self, message: &Message, substring: bool) -> usize {
        let mut messages = self.messages.lock();
        let mut count = 0;
        for (m, checked) in messages.iter_mut() {
            if !*checked
                && m.severity == message.severity
                && m.event == message.event
                && m.code == message.code
                && (if substring { m.msg.contains(&message.msg) } else { m.msg == message.msg })
            {
                *checked = true;
                count += 1;
            }
        }
        count
    }

    pub(crate) fn unchecked_count(&self) -> usize {
        self.messages.lock().iter().filter(|(_, checked)| !checked).count()
    }

    pub(crate) fn unchecked(&self) -> Vec<Message> {
        self.messages.lock().iter().filter(|(_, checked)| !checked).map(|(m, _)| m.clone()).collect()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

pub(crate) fn error(code: i32, msg: &str) -> Message {
    Message { severity: EventSeverity::Error, event: Event::Database, code: i64::from(code), msg: msg.to_owned() }
}

pub(crate) fn warning(code: i64, msg: &str) -> Message {
    Message { severity: EventSeverity::Warning, event: Event::Database, code, msg: msg.to_owned() }
}

/// A Mersenne Twister (`std::mt19937`, default seed 5489), so `random_string`
/// gives the C++ tests' strings.
pub(crate) struct Mt19937 {
    mt: [u32; 624],
    index: usize,
}

impl Mt19937 {
    pub(crate) fn new(seed: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            mt[i] = 1_812_433_253_u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Mt19937 { mt, index: 624 }
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
                let mut v = self.mt[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    v ^= 0x9908_b0df;
                }
                self.mt[i] = v;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

/// `size` pseudo-random bytes: the low byte of each `Mt19937` draw.
pub(crate) fn random_string(size: usize) -> crate::response::Data {
    let mut random = Mt19937::new(5489);
    Arc::new((0..size).map(|_| random.next_u32() as u8).collect())
}

/// Copies `source` to `destination`. A directory as the source copies
/// nothing, as in C++.
pub(crate) fn copy_file(destination: &str, source: &std::path::Path) {
    let mut src = std::fs::File::open(source).unwrap_or_else(|e| panic!("Cannot read file {}: {e}", source.display()));
    let mut dst = std::fs::File::create(destination).unwrap_or_else(|e| panic!("Cannot write file {destination}: {e}"));
    let _ = std::io::copy(&mut src, &mut dst);
}

/// Deletes `filename` for test cleanup; a missing file is fine.
pub(crate) fn delete_file(filename: &str) {
    let _ = std::fs::remove_file(filename);
}

pub(crate) fn file_size(filename: &str) -> usize {
    std::fs::read(filename).unwrap_or_else(|e| panic!("Cannot read file {filename}: {e}")).len()
}

#[test]
fn mt19937_matches_the_standard() {
    // The C++ standard requires the 10000th draw of a default-constructed
    // std::mt19937 to be 4123659995.
    let mut random = Mt19937::new(5489);
    let mut value = 0;
    for _ in 0..10000 {
        value = random.next_u32();
    }
    assert_eq!(4_123_659_995, value);
}

thread_local! {
    static LOCAL_CALLBACKS: RefCell<std::collections::HashMap<u64, Box<dyn FnOnce()>>> =
        RefCell::new(std::collections::HashMap::new());
}

/// A `Send` handle for a closure that runs on the calling thread only. The
/// C++ tests pass callbacks that capture references to `forward`, which runs
/// them on the calling thread (`Scheduler::bindOnce`); the Rust signature
/// requires `Send`, so the closure stays here and the handle finds it.
pub(crate) fn local_callback(f: impl FnOnce() + 'static) -> Box<dyn FnOnce() + Send> {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    LOCAL_CALLBACKS.with(|callbacks| callbacks.borrow_mut().insert(id, Box::new(f)));
    let thread = std::thread::current().id();
    Box::new(move || {
        assert_eq!(thread, std::thread::current().id(), "local callback called on another thread");
        let f = LOCAL_CALLBACKS.with(|callbacks| callbacks.borrow_mut().remove(&id));
        if let Some(f) = f {
            f();
        }
    })
}
// Ports test/util/http_timeout.test.cpp (marker `// gtest: Group.Name`).
mod gtest_http_timeout;
