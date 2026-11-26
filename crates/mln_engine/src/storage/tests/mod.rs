// Ported gtests (test/storage/*.test.cpp) and test helpers.

#![cfg_attr(not(feature = "sqlite"), expect(dead_code, reason = "most helpers serve the SQLite tests"))]

mod asset_file_source;
#[cfg(feature = "sqlite")]
mod database_file_source;
mod http_file_source;
mod local_file_source;
// The C++ MainResourceLoader tests assume its Database file source.
#[cfg(feature = "sqlite")]
mod main_resource_loader;
#[cfg(feature = "sqlite")]
mod mbtiles_file_source;
mod network;
mod offline;
#[cfg(feature = "sqlite")]
mod offline_database;
mod online_file_source;
mod pmtiles_file_source;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Once, mpsc};

use mln_style::hash::HashMap;
use mln_util::queue::{Queue, WeakQueue};
use parking_lot::Mutex;

use crate::storage::Request;
use crate::storage::file_source::{FileSource, FileSourceExt};
use crate::storage::resource::Resource;
use crate::storage::response::Response;

/// A request that its own handler can drop.
pub(crate) type Req = Rc<RefCell<Option<LocalRequest>>>;

pub(crate) fn req() -> Req {
    Rc::new(RefCell::new(None))
}

type Handler = Rc<RefCell<Box<dyn FnMut(Response)>>>;

/// The test thread's loop: a queue whose tasks run with the loop's
/// [`Dispatcher`], so that responses reach handlers that aren't `Send` on
/// the test's thread. Clones share the loop.
#[derive(Clone)]
pub(crate) struct TestLoop {
    queue: Queue<Dispatcher>,
    dispatcher: Dispatcher,
}

impl TestLoop {
    pub(crate) fn new() -> Self {
        let queue = Queue::new();
        let dispatcher = Dispatcher {
            inner: Rc::new(DispatcherInner {
                queue: queue.downgrade(),
                handlers: RefCell::default(),
                callbacks: RefCell::default(),
                next_token: Cell::new(0),
            }),
        };
        TestLoop { queue, dispatcher }
    }

    /// The dispatcher whose responses run in the loop's turns.
    pub(crate) fn dispatcher(&self) -> Dispatcher {
        self.dispatcher.clone()
    }

    /// A handle that other threads post to and stop the loop with.
    pub(crate) fn handle(&self) -> Queue<Dispatcher> {
        self.queue.clone()
    }

    /// Runs the loop until [`stop`](Self::stop).
    pub(crate) fn run(&self) {
        self.queue.run(&mut self.dispatcher.clone());
    }

    /// Runs what waits now, and returns whether anything ran.
    pub(crate) fn run_once(&self) -> bool {
        self.queue.run_once(&mut self.dispatcher.clone())
    }

    pub(crate) fn stop(&self) {
        self.queue.stop();
    }
}

/// Runs requests' responses on the test's thread, in the turns of its
/// [`TestLoop`], for handlers that aren't `Send`. Responses run in the order
/// they arrive, and none runs after its request drops. Clones share the
/// dispatcher.
#[derive(Clone)]
pub(crate) struct Dispatcher {
    inner: Rc<DispatcherInner>,
}

struct DispatcherInner {
    queue: WeakQueue<Dispatcher>,
    handlers: RefCell<HashMap<u64, Handler>>,
    /// The callbacks that [`Dispatcher::callback`] made and that haven't run.
    callbacks: RefCell<HashMap<u64, Box<dyn FnOnce()>>>,
    next_token: Cell<u64>,
}

impl Dispatcher {
    fn next_token(&self) -> u64 {
        let token = self.inner.next_token.get();
        self.inner.next_token.set(token + 1);
        token
    }

    /// Hands `response` to the handler of request `token`, unless the
    /// request dropped.
    fn dispatch(&self, token: u64, response: Response) {
        let handler = self.inner.handlers.borrow().get(&token).cloned();
        if let Some(handler) = handler {
            (handler.borrow_mut())(response);
        }
    }

    /// A `Send` callback that runs `f` on the test's thread, in a turn of
    /// its loop, when it's called from any thread.
    pub(crate) fn callback(&self, f: impl FnOnce() + 'static) -> Box<dyn FnOnce() + Send> {
        let token = self.next_token();
        self.inner.callbacks.borrow_mut().insert(token, Box::new(f));
        let queue = self.inner.queue.clone();
        Box::new(move || {
            if let Some(queue) = queue.upgrade() {
                queue.post(move |dispatcher| {
                    let f = dispatcher.inner.callbacks.borrow_mut().remove(&token);
                    if let Some(f) = f {
                        f();
                    }
                });
            }
        })
    }
}

/// A request whose responses a [`Dispatcher`] hands to its handler, which
/// it keeps alive. Dropping it cancels the request.
pub(crate) struct LocalRequest {
    token: u64,
    dispatcher: Dispatcher,
    _request: Request,
}

impl Drop for LocalRequest {
    fn drop(&mut self) {
        let handler = self.dispatcher.inner.handlers.borrow_mut().remove(&self.token);
        drop(handler);
    }
}

/// Requests whose handlers run on the test's thread.
pub(crate) trait LocalRequests: FileSource {
    /// Requests `resource`; `dispatcher` hands the responses to `handler`.
    fn request_local(
        &self,
        dispatcher: &Dispatcher,
        resource: &Resource,
        handler: impl FnMut(Response) + 'static,
    ) -> LocalRequest {
        let token = dispatcher.next_token();
        dispatcher.inner.handlers.borrow_mut().insert(token, Rc::new(RefCell::new(Box::new(handler))));
        let queue = dispatcher.inner.queue.clone();
        let request = self.request(resource, move |response| {
            if let Some(queue) = queue.upgrade() {
                queue.post(move |dispatcher| dispatcher.dispatch(token, response));
            }
        });
        LocalRequest { token, dispatcher: dispatcher.clone(), _request: request }
    }
}

impl<T: FileSource + ?Sized> LocalRequests for T {}

/// Holds a worker's thread in the task that `send` sends it, after the
/// work already sent to it, until the returned guard drops. It returns once
/// the thread is held, so the work sent meanwhile queues up and runs in one
/// go after the drop. Drop the guard before the worker's owner, whose drop
/// waits for the worker.
pub(crate) fn pause(send: impl FnOnce(Box<dyn FnOnce() + Send>)) -> Paused {
    let (paused, is_paused) = mpsc::channel();
    let (resume, resumed) = mpsc::channel::<()>();
    send(Box::new(move || {
        let _ = paused.send(());
        // Returns once `Paused` drops its sender.
        let _ = resumed.recv();
    }));
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
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative)
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
    pub(crate) level: log::Level,
    pub(crate) target: &'static str,
    pub(crate) code: i64,
    pub(crate) msg: String,
}

/// A record the test logger caught: its level, target, code and message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Caught {
    pub(crate) level: log::Level,
    pub(crate) target: String,
    pub(crate) code: i64,
    pub(crate) msg: String,
}

type Messages = Arc<Mutex<Vec<(Caught, bool)>>>;

/// A function that sees the records of every thread, and returns whether it
/// took the record.
type Watch = Box<dyn Fn(&Caught) -> bool + Send>;

thread_local! {
    /// The messages of the thread's live `FixtureLog`, if any.
    static CAUGHT: RefCell<Option<Messages>> = const { RefCell::new(None) };
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// The test binary's `log` logger. It hands each of the library's records to
/// the process's [`watch`], and those that it doesn't take to the
/// `FixtureLog` of the thread that logged them, so tests that run on threads
/// of one process don't see each other's.
struct Catcher;

impl log::Log for Catcher {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("mln_")
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let code = record.key_values().get(log::kv::Key::from_str("code")).and_then(|code| code.to_i64());
        let caught = Caught {
            level: record.level(),
            target: record.target().to_owned(),
            code: code.unwrap_or(-1),
            msg: record.args().to_string(),
        };
        if WATCH.lock().as_ref().is_some_and(|watch| watch(&caught)) {
            return;
        }
        CAUGHT.with_borrow(|messages| {
            if let Some(messages) = messages {
                messages.lock().push((caught, false));
            }
        });
    }

    fn flush(&self) {}
}

fn install_logger() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Catcher).expect("the test binary has no other logger");
        log::set_max_level(log::LevelFilter::Info);
    });
}

/// Hands every thread's records to `watch` until the guard drops.
pub(crate) fn watch(watch: impl Fn(&Caught) -> bool + Send + 'static) -> impl Drop {
    struct Unwatch;
    impl Drop for Unwatch {
        fn drop(&mut self) {
            WATCH.lock().take();
        }
    }
    install_logger();
    *WATCH.lock() = Some(Box::new(watch));
    Unwatch
}

/// Records the calling thread's log messages for a test to check.
pub(crate) struct FixtureLog {
    messages: Messages,
}

impl FixtureLog {
    pub(crate) fn new() -> Self {
        install_logger();
        let messages = Messages::default();
        CAUGHT.set(Some(Arc::clone(&messages)));
        FixtureLog { messages }
    }

    /// `count(message, substring)`: marks the matches checked.
    pub(crate) fn count(&self, message: &Message, substring: bool) -> usize {
        let mut messages = self.messages.lock();
        let mut count = 0;
        for (m, checked) in messages.iter_mut() {
            if !*checked
                && m.level == message.level
                && m.target == message.target
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

    pub(crate) fn unchecked(&self) -> Vec<Caught> {
        self.messages.lock().iter().filter(|(_, checked)| !checked).map(|(m, _)| m.clone()).collect()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        CAUGHT.set(None);
    }
}

pub(crate) fn error(code: i32, msg: &str) -> Message {
    Message {
        level: log::Level::Error,
        target: crate::log_target::DATABASE,
        code: i64::from(code),
        msg: msg.to_owned(),
    }
}

pub(crate) fn warning(code: i64, msg: &str) -> Message {
    Message { level: log::Level::Warn, target: crate::log_target::DATABASE, code, msg: msg.to_owned() }
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
pub(crate) fn random_string(size: usize) -> crate::storage::response::Data {
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

/// A pool for a test's file sources.
pub(crate) fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4)
}

// Ports test/util/http_timeout.test.cpp (marker `// gtest: Group.Name`).
mod gtest_http_timeout;
