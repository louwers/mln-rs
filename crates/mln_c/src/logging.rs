//! `logging.h`: the process's log records, sent to a host's callback.
//!
//! The library logs through the `log` crate. This module installs the
//! process's `log` logger, [`Bridge`], which forwards records to the host's
//! handler: on the thread that logged them, or, for the severities of the
//! async mask, on a log thread of its own, in the order they were logged. A
//! record that no handler takes, or whose severity the handler's platform
//! severities name, goes to the platform's log.
//!
//! A library crate names a record's event in its target, as
//! `<crate>::<event>`: `mln_style::parse_style` is
//! `MLN_LOG_EVENT_PARSE_STYLE` (see [`EVENTS`]). Other targets are general,
//! but wgpu's and naga's, which are about the graphics API. A record's
//! `code` value, such as an SQLite result code, is its code.

use core::cell::Cell;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Once, mpsc};

use parking_lot::{Mutex, RwLock};

use crate::abi::{self, Fields};
use crate::buffer::LentText;
use crate::callback::Registration;
use crate::error::Error;
use crate::export::ForbidReentry;
use crate::generated::types::{
    MLN_LOG_EVENT_CRASH, MLN_LOG_EVENT_DATABASE, MLN_LOG_EVENT_GENERAL, MLN_LOG_EVENT_GLYPH,
    MLN_LOG_EVENT_GRAPHICS_BACKEND, MLN_LOG_EVENT_HTTP_REQUEST, MLN_LOG_EVENT_IMAGE, MLN_LOG_EVENT_PARSE_STYLE,
    MLN_LOG_EVENT_PARSE_TILE, MLN_LOG_EVENT_RENDER, MLN_LOG_EVENT_SETUP, MLN_LOG_EVENT_SPRITE, MLN_LOG_EVENT_STYLE,
    MLN_LOG_EVENT_TIMING, MLN_LOG_SEVERITY_ERROR, MLN_LOG_SEVERITY_FLAG_ALL, MLN_LOG_SEVERITY_FLAG_DEFAULT,
    MLN_LOG_SEVERITY_INFO, MLN_LOG_SEVERITY_WARNING, mln_buffer_view, mln_diagnostic, mln_log_event, mln_log_handler,
    mln_log_severity, mln_log_severity_flags, mln_status,
};

abi::versioned!(mln_log_handler);

/// A log callback can't call the library: the record's thread may hold this
/// module's lock.
const REENTRY: &str = "the C API cannot be called from a log callback, wake, or release";

type Callback = unsafe extern "C" fn(*mut core::ffi::c_void, mln_log_severity, mln_log_event, i64, mln_buffer_view);

struct Handler {
    callback: Callback,
    registration: Registration,
    platform_severities: mln_log_severity_flags,
}

/// The host's handler. Calls hold it for reading, so a replacement waits for
/// the calls that are running and no call runs after it returns.
static HANDLER: RwLock<Option<Handler>> = RwLock::new(None);

/// The severities that go to the log thread.
static ASYNC_SEVERITIES: AtomicU32 = AtomicU32::new(MLN_LOG_SEVERITY_FLAG_DEFAULT);

thread_local! {
    /// Whether this thread is in a log callback, where a record that the
    /// host's code logs goes to the platform's log instead.
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// The events that the library's targets name: each target's name for it,
/// its value, and its name in the platform's log.
const EVENTS: [(&str, mln_log_event, &str); 14] = [
    ("general", MLN_LOG_EVENT_GENERAL, "General"),
    ("setup", MLN_LOG_EVENT_SETUP, "Setup"),
    ("parse_style", MLN_LOG_EVENT_PARSE_STYLE, "ParseStyle"),
    ("parse_tile", MLN_LOG_EVENT_PARSE_TILE, "ParseTile"),
    ("render", MLN_LOG_EVENT_RENDER, "Render"),
    ("style", MLN_LOG_EVENT_STYLE, "Style"),
    ("database", MLN_LOG_EVENT_DATABASE, "Database"),
    ("http_request", MLN_LOG_EVENT_HTTP_REQUEST, "HttpRequest"),
    ("sprite", MLN_LOG_EVENT_SPRITE, "Sprite"),
    ("image", MLN_LOG_EVENT_IMAGE, "Image"),
    ("graphics_backend", MLN_LOG_EVENT_GRAPHICS_BACKEND, "GraphicsBackend"),
    ("crash", MLN_LOG_EVENT_CRASH, "Crash"),
    ("glyph", MLN_LOG_EVENT_GLYPH, "Glyph"),
    ("timing", MLN_LOG_EVENT_TIMING, "Timing"),
];

/// One record, as the log thread carries it.
struct Record {
    severity: mln_log_severity,
    event: mln_log_event,
    code: i64,
    message: String,
}

/// The severity of a record of `level`; debug records have none, and go to
/// the platform's log only.
fn severity(level: log::Level) -> Option<mln_log_severity> {
    match level {
        log::Level::Error => Some(MLN_LOG_SEVERITY_ERROR),
        log::Level::Warn => Some(MLN_LOG_SEVERITY_WARNING),
        log::Level::Info => Some(MLN_LOG_SEVERITY_INFO),
        log::Level::Debug | log::Level::Trace => None,
    }
}

/// The severity's bit in a set of severities.
fn flag(severity: mln_log_severity) -> mln_log_severity_flags {
    1 << severity
}

/// Sends `record` to the host's handler, if there is one.
fn deliver(record: &Record) {
    let handler = HANDLER.read();
    let Some(handler) = &*handler else { return };
    let callback = handler.callback;
    let _forbid = ForbidReentry::enter(REENTRY);
    let previous = IN_CALLBACK.replace(true);
    let message = LentText::new(&record.message);
    let Record { severity, event, code, .. } = *record;
    // A record that the handler's released queue refuses is dropped.
    let _ = handler.registration.deliver(move |user_data| {
        // SAFETY: the host's callback, with its user data and a message that
        // lives until it returns.
        unsafe { callback(user_data.get(), severity, event, code, message.view()) };
    });
    IN_CALLBACK.set(previous);
}

/// The log thread's queue, once it has started.
static LOG_THREAD: Mutex<Option<mpsc::Sender<Record>>> = Mutex::new(None);

/// Sends `record` to the log thread, or returns it when the thread can't
/// start.
fn deliver_later(record: Record) -> Result<(), Record> {
    // The browser's instance has no other thread.
    if cfg!(target_family = "wasm") {
        return Err(record);
    }
    let mut thread = LOG_THREAD.lock();
    if thread.is_none() {
        let (sender, receiver) = mpsc::channel::<Record>();
        let started = mln_util::thread::builder("mln-log")
            .spawn(move || receiver.into_iter().for_each(|record| deliver(&record)));
        if started.is_err() {
            return Err(record);
        }
        *thread = Some(sender);
    }
    match &*thread {
        Some(sender) => sender.send(record).map_err(|mpsc::SendError(record)| record),
        None => Err(record),
    }
}

/// Hands a record to the host's handler, if there is one, now or on the log
/// thread. Returns whether the record stays out of the platform's log.
fn forward(severity: mln_log_severity, event: mln_log_event, code: i64, message: &str) -> bool {
    if IN_CALLBACK.get() {
        return false;
    }
    let platform = {
        let handler = HANDLER.read();
        match &*handler {
            Some(handler) => handler.platform_severities & flag(severity) != 0,
            None => return false,
        }
    };
    let record = Record { severity, event, code, message: message.to_owned() };
    if ASYNC_SEVERITIES.load(Ordering::Acquire) & flag(severity) != 0 {
        if let Err(record) = deliver_later(record) {
            deliver(&record);
        }
    } else {
        deliver(&record);
    }
    !platform
}

/// Writes a record to the platform's log, stderr.
fn platform_record(level: log::Level, event: &str, code: i64, message: &str) {
    eprintln!("{}", platform_line(level, event, code, message));
}

/// A record's line in the platform's log.
fn platform_line(level: log::Level, event: &str, code: i64, message: &str) -> String {
    let severity = match level {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARNING",
        log::Level::Info => "INFO",
        log::Level::Debug | log::Level::Trace => "DEBUG",
    };
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or_default();
    let code = if code >= 0 { format!("({code})") } else { String::new() };
    let separator = if message.is_empty() { "" } else { ": " };
    format!("[{severity}] {{{thread}}}[{event}]{code}{separator}{message}")
}

/// The process's `log` logger: it hands records to the host's handler, or to
/// the platform's log.
///
/// It takes the library's own records at every severity but debug, which
/// it prints in debug builds only, and other crates' warnings and errors:
/// what wgpu and naga log below that is detail about the graphics API that C
/// hosts haven't asked for.
struct Bridge;

impl log::Log for Bridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let floor = match library_crate(metadata.target()) {
            Some(_) if cfg!(debug_assertions) => log::Level::Debug,
            Some(_) => log::Level::Info,
            None => log::Level::Warn,
        };
        metadata.level() <= floor
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let (event, name) = event(record.target());
        let code = record.key_values().get(log::kv::Key::from_str("code")).and_then(|code| code.to_i64());
        let code = code.unwrap_or(-1);
        let message = record.args().as_str().map_or_else(|| Cow::Owned(record.args().to_string()), Cow::Borrowed);
        if severity(record.level()).is_some_and(|severity| forward(severity, event, code, &message)) {
            return;
        }
        platform_record(record.level(), name, code, &message);
    }

    fn flush(&self) {}
}

/// The rest of `target` after its crate's name, if the crate is one of the
/// library's.
fn library_crate(target: &str) -> Option<&str> {
    let (krate, rest) = target.split_once("::").unwrap_or((target, ""));
    krate.starts_with("mln_").then_some(rest)
}

/// The event a `log` record reports, by its target, and its name in the
/// platform's log.
fn event(target: &str) -> (mln_log_event, &'static str) {
    let name = match library_crate(target) {
        Some(event) => event,
        None if target.starts_with("wgpu") || target.starts_with("naga") => "graphics_backend",
        None => "general",
    };
    let (_, event, name) = EVENTS.iter().find(|(known, ..)| *known == name).unwrap_or(&EVENTS[0]);
    (*event, name)
}

/// Makes [`Bridge`] the process's `log` logger, once, before the library
/// first logs: when an entry point first runs. A Rust program that links the
/// library and installed a logger of its own keeps it, and reads the
/// library's records there.
pub(crate) fn install_logger() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        if log::set_logger(&Bridge).is_ok() {
            log::set_max_level(if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info });
        }
    });
}

/// Replaces the handler, and releases the one it replaces as a log callback's
/// release runs: unable to call the library.
fn replace(handler: Option<Handler>) {
    let previous = core::mem::replace(&mut *HANDLER.write(), handler);
    let _forbid = ForbidReentry::enter(REENTRY);
    drop(previous);
}

/// `mln_log_set_callback()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_log_set_callback(
    handler: *const mln_log_handler,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        if handler.is_null() {
            return Err(Error::invalid_argument("log handler must not be null"));
        }
        // SAFETY: the caller's contract.
        let handler = unsafe { abi::read_sized(handler) }?;
        Fields::new(handler.platform_severities, MLN_LOG_SEVERITY_FLAG_ALL, "log handler platform_severities")?;
        let Some(callback) = handler.callback else {
            return Err(Error::invalid_argument("log handler requires a callback"));
        };
        let registration =
            Registration::new("log handler", true, handler.user_data, handler.release_user_data, handler.queue)?;
        registration.accept();
        // Before the log thread can call the host.
        crate::process_exit::watch();
        replace(Some(Handler { callback, registration, platform_severities: handler.platform_severities }));
        Ok(())
    })
}

/// `mln_log_clear_callback()`.
///
/// # Safety
///
/// `out_diagnostic` is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_log_clear_callback(out_diagnostic: *mut mln_diagnostic) -> mln_status {
    export!(out_diagnostic, {
        replace(None);
        Ok(())
    })
}

/// `mln_log_set_async_severity_mask()`.
///
/// # Safety
///
/// `out_diagnostic` is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_log_set_async_severity_mask(
    mask: mln_log_severity_flags,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        Fields::new(mask, MLN_LOG_SEVERITY_FLAG_ALL, "log async severity mask")?;
        ASYNC_SEVERITIES.store(mask, Ordering::Release);
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use core::ffi::c_void;
    use std::sync::Arc;
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    use parking_lot::MutexGuard;

    use super::*;
    use crate::delivery::tests::queue;
    use crate::delivery::{mln_delivery_queue_dispatch, mln_delivery_queue_release};
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{
        MLN_LOG_SEVERITY_FLAG_ERROR, MLN_LOG_SEVERITY_FLAG_INFO, MLN_LOG_SEVERITY_FLAG_WARNING,
        MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_INVALID_STATE, MLN_STATUS_OK, mln_delivery_queue,
    };
    use crate::gpu_device::{mln_gpu_device_create, mln_gpu_device_release};

    const NULL_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

    /// The log state is the process's, so its tests take turns, and each
    /// restores the defaults when it ends.
    pub(crate) struct LogState(#[expect(dead_code, reason = "held for its lifetime")] MutexGuard<'static, ()>);

    impl LogState {
        pub(crate) fn take() -> Self {
            static TURN: Mutex<()> = Mutex::new(());
            Self(TURN.lock())
        }
    }

    impl Drop for LogState {
        fn drop(&mut self) {
            // SAFETY: null diagnostics.
            unsafe {
                mln_log_clear_callback(NULL_DIAGNOSTIC);
                mln_log_set_async_severity_mask(MLN_LOG_SEVERITY_FLAG_DEFAULT, NULL_DIAGNOSTIC);
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct Seen {
        pub(crate) severity: mln_log_severity,
        pub(crate) event: mln_log_event,
        pub(crate) code: i64,
        pub(crate) message: String,
        pub(crate) thread: ThreadId,
    }

    /// What a handler saw: the records with this test's marker, the releases,
    /// and the status of a call back into the library, if it made one.
    #[derive(Default)]
    pub(crate) struct Probe {
        marker: String,
        records: Mutex<Vec<Seen>>,
        releases: AtomicU32,
        reentry: Mutex<Option<(mln_status, String)>>,
        call_back: bool,
    }

    impl Probe {
        pub(crate) fn new(marker: &str, call_back: bool) -> Arc<Self> {
            Arc::new(Self { marker: marker.to_owned(), call_back, ..Self::default() })
        }

        pub(crate) fn records(&self) -> Vec<Seen> {
            self.records.lock().clone()
        }

        fn wait_for(&self, count: usize) -> Vec<Seen> {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.records().len() < count && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.records()
        }

        fn clear_from_callback() -> (mln_status, String) {
            let mut out = diagnostic();
            // SAFETY: a local diagnostic.
            let status = unsafe { mln_log_clear_callback(&raw mut *out) };
            (status, message(&out).to_owned())
        }
    }

    unsafe extern "C" fn record(
        user_data: *mut c_void,
        severity: mln_log_severity,
        event: mln_log_event,
        code: i64,
        message: mln_buffer_view,
    ) {
        // SAFETY: every test's user data is a `Probe` from `handler`, which
        // it leaks.
        let probe = unsafe { &*user_data.cast::<Probe>() };
        // SAFETY: the library's message lives until the callback returns.
        let bytes = unsafe { core::slice::from_raw_parts(message.data.cast::<u8>(), message.size) };
        let message = String::from_utf8_lossy(bytes).into_owned();
        if !message.contains(&probe.marker) {
            return;
        }
        if probe.call_back {
            *probe.reentry.lock() = Some(Probe::clear_from_callback());
        }
        let thread = std::thread::current().id();
        probe.records.lock().push(Seen { severity, event, code, message, thread });
    }

    unsafe extern "C" fn release(user_data: *mut c_void) {
        // SAFETY: as in `record`.
        let probe = unsafe { &*user_data.cast::<Probe>() };
        probe.releases.fetch_add(1, Ordering::SeqCst);
        if probe.call_back {
            *probe.reentry.lock() = Some(Probe::clear_from_callback());
        }
    }

    pub(crate) fn handler(probe: &Arc<Probe>, queue: mln_delivery_queue) -> mln_log_handler {
        mln_log_handler {
            size: size_of::<mln_log_handler>() as u32,
            callback: Some(record),
            user_data: Arc::into_raw(Arc::clone(probe)).cast_mut().cast(),
            release_user_data: Some(release),
            queue,
            platform_severities: 0,
        }
    }

    pub(crate) fn set(handler: &mln_log_handler) -> mln_status {
        // SAFETY: a local handler and a null diagnostic.
        unsafe { mln_log_set_callback(handler, NULL_DIAGNOSTIC) }
    }

    pub(crate) fn set_async(mask: mln_log_severity_flags) -> mln_status {
        // SAFETY: a null diagnostic.
        unsafe { mln_log_set_async_severity_mask(mask, NULL_DIAGNOSTIC) }
    }

    fn clear() -> mln_status {
        // SAFETY: a null diagnostic.
        unsafe { mln_log_clear_callback(NULL_DIAGNOSTIC) }
    }

    #[test]
    fn a_handler_releases_its_user_data_once_when_replaced_or_cleared() {
        let _state = LogState::take();
        let (first, second) = (Probe::new("release", false), Probe::new("release", false));
        assert_eq!(set(&handler(&first, 0)), MLN_STATUS_OK);
        assert_eq!(first.releases.load(Ordering::SeqCst), 0);
        assert_eq!(set(&handler(&second, 0)), MLN_STATUS_OK);
        assert_eq!(first.releases.load(Ordering::SeqCst), 1);
        assert_eq!(clear(), MLN_STATUS_OK);
        assert_eq!(clear(), MLN_STATUS_OK);
        assert_eq!(second.releases.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_rejected_handler_keeps_the_installed_one() {
        let _state = LogState::take();
        let (installed, rejected) = (Probe::new("kept", false), Probe::new("kept", false));
        assert_eq!(set(&handler(&installed, 0)), MLN_STATUS_OK);
        // SAFETY: a null handler and diagnostic.
        assert_eq!(unsafe { mln_log_set_callback(core::ptr::null(), NULL_DIAGNOSTIC) }, MLN_STATUS_INVALID_ARGUMENT);
        let small = mln_log_handler { size: size_of::<mln_log_handler>() as u32 - 1, ..handler(&rejected, 0) };
        let silent = mln_log_handler { callback: None, ..handler(&rejected, 0) };
        let unknown = mln_log_handler { platform_severities: 1, ..handler(&rejected, 0) };
        let past = mln_log_handler { platform_severities: 1 << 4, ..handler(&rejected, 0) };
        for rejected in [small, silent, unknown, past] {
            assert_eq!(set(&rejected), MLN_STATUS_INVALID_ARGUMENT);
        }
        log::warn!(target: "mln_c::general", "kept");
        assert_eq!(installed.wait_for(1).len(), 1);
        assert_eq!(rejected.releases.load(Ordering::SeqCst), 0);
        assert_eq!(clear(), MLN_STATUS_OK);
        assert_eq!(installed.releases.load(Ordering::SeqCst), 1);
        assert_eq!(rejected.releases.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_async_mask_accepts_only_severity_bits() {
        let _state = LogState::take();
        assert_eq!(set_async(MLN_LOG_SEVERITY_FLAG_ALL), MLN_STATUS_OK);
        assert_eq!(set_async(0), MLN_STATUS_OK);
        for mask in [1, 1 << 4] {
            let mut out = diagnostic();
            // SAFETY: a local diagnostic.
            assert_eq!(unsafe { mln_log_set_async_severity_mask(mask, &raw mut *out) }, MLN_STATUS_INVALID_ARGUMENT);
            assert!(message(&out).contains("unknown bits"), "{}", message(&out));
        }
    }

    #[test]
    fn records_arrive_on_the_logging_thread_or_in_order_on_the_log_thread() {
        let _state = LogState::take();
        let probe = Probe::new("threads", false);
        assert_eq!(set_async(MLN_LOG_SEVERITY_FLAG_WARNING), MLN_STATUS_OK);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        log::info!(target: "mln_c::timing", "threads: info");
        log::warn!(target: "mln_c::general", "threads: first");
        log::warn!(target: "mln_c::general", "threads: second");
        log::error!(target: "mln_c::general", "threads: error");
        let records = probe.wait_for(4);
        let here = std::thread::current().id();
        let sync: Vec<_> = records.iter().filter(|seen| seen.thread == here).collect();
        assert_eq!(
            sync.iter().map(|seen| seen.severity).collect::<Vec<_>>(),
            [MLN_LOG_SEVERITY_INFO, MLN_LOG_SEVERITY_ERROR]
        );
        assert_eq!(
            (sync[0].event, sync[0].code, sync[0].message.as_str()),
            (MLN_LOG_EVENT_TIMING, -1, "threads: info")
        );
        let later: Vec<_> = records.iter().filter(|seen| seen.thread != here).collect();
        assert_eq!(
            later.iter().map(|seen| seen.message.as_str()).collect::<Vec<_>>(),
            ["threads: first", "threads: second"]
        );
        assert_eq!(later[0].thread, later[1].thread);
        assert_eq!((later[0].severity, later[0].event), (MLN_LOG_SEVERITY_WARNING, MLN_LOG_EVENT_GENERAL));
    }

    #[test]
    fn a_map_command_without_a_completion_logs_its_failure() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let probe = Probe::new("discarded command failed", false);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        let runtime = crate::runtime::tests::create();
        let map = crate::map::tests::create_map(runtime, &crate::map::tests::options(1.0));
        // SAFETY: a null completion and diagnostic.
        let status = unsafe { crate::map::mln_map_end_command_group(map, core::ptr::null(), NULL_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        let records = probe.wait_for(1);
        crate::map::tests::release_map(map);
        crate::runtime::tests::release(runtime);
        assert_eq!(
            records.iter().map(|seen| (seen.severity, seen.message.as_str())).collect::<Vec<_>>(),
            [(
                MLN_LOG_SEVERITY_WARNING,
                format!(
                    "discarded command failed (status {MLN_STATUS_INVALID_STATE}): the map has no open command group"
                )
                .as_str()
            )]
        );
    }

    #[test]
    fn a_log_callback_and_its_release_cannot_call_back_into_the_library() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let probe = Probe::new("reentry", true);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        log::warn!(target: "mln_c::general", "reentry");
        let (status, text) = probe.reentry.lock().take().unwrap();
        assert_eq!(status, MLN_STATUS_INVALID_STATE);
        assert!(text.contains("log callback"), "{text}");
        // The callback's call changed nothing.
        log::warn!(target: "mln_c::general", "reentry again");
        assert_eq!(probe.records().len(), 2);

        assert_eq!(clear(), MLN_STATUS_OK);
        assert_eq!(probe.releases.load(Ordering::SeqCst), 1);
        assert_eq!(probe.reentry.lock().take().map(|(status, _)| status), Some(MLN_STATUS_INVALID_STATE));
    }

    #[test]
    fn a_cleared_handler_receives_no_more_records() {
        let _state = LogState::take();
        let probe = Probe::new("cleared", false);
        assert_eq!(set_async(MLN_LOG_SEVERITY_FLAG_ALL), MLN_STATUS_OK);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        log::error!(target: "mln_c::general", "cleared: before");
        assert_eq!(probe.wait_for(1).len(), 1);
        assert_eq!(clear(), MLN_STATUS_OK);
        log::error!(target: "mln_c::general", "cleared: after");
        // The log thread delivers in order, so once another handler sees a
        // later record, the cleared one would have seen this one.
        let next = Probe::new("cleared", false);
        assert_eq!(set(&handler(&next, 0)), MLN_STATUS_OK);
        log::error!(target: "mln_c::general", "cleared: next");
        assert_eq!(next.wait_for(1).len(), 1);
        assert_eq!(probe.records().len(), 1);
    }

    #[test]
    fn a_queued_handler_receives_records_in_dispatch() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let queue = queue();
        let probe = Probe::new("queued", true);
        assert_eq!(set(&handler(&probe, queue)), MLN_STATUS_OK);
        log::info!(target: "mln_c::general", "queued");
        assert_eq!(probe.records(), Vec::<Seen>::new());
        // SAFETY: a null diagnostic.
        assert_eq!(unsafe { mln_delivery_queue_dispatch(queue, NULL_DIAGNOSTIC) }, MLN_STATUS_OK);
        assert_eq!(probe.records().len(), 1);
        // From the queue's dispatch, the callback may call the library.
        assert_eq!(probe.reentry.lock().take().map(|(status, _)| status), Some(MLN_STATUS_OK));
        mln_delivery_queue_release(queue);
    }

    #[test]
    fn the_platform_severities_go_to_the_platform_log_too() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let probe = Probe::new("platform", false);
        let platform = MLN_LOG_SEVERITY_FLAG_ERROR | MLN_LOG_SEVERITY_FLAG_INFO;
        assert_eq!(set(&mln_log_handler { platform_severities: platform, ..handler(&probe, 0) }), MLN_STATUS_OK);
        // The bridge prints the records that `forward` doesn't keep out.
        let kept_out = |severity| forward(severity, MLN_LOG_EVENT_GENERAL, -1, "platform");
        assert!(!kept_out(MLN_LOG_SEVERITY_INFO));
        assert!(kept_out(MLN_LOG_SEVERITY_WARNING));
        assert!(!kept_out(MLN_LOG_SEVERITY_ERROR));
        assert_eq!(probe.records().len(), 3);
        assert_eq!(clear(), MLN_STATUS_OK);
        assert!(!kept_out(MLN_LOG_SEVERITY_WARNING));
    }

    #[test]
    fn log_records_arrive_under_the_event_their_target_names() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let probe = Probe::new("bridge", false);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        log::info!(target: "mln_gl::render", "bridge: render");
        log::error!(target: "mln_gl::graphics_backend", "bridge: {}", "backend");
        log::warn!(target: "mln_style::parse_style", "bridge: style");
        log::error!(target: "mln_storage::database", code = 11; "bridge: database");
        log::info!(target: "mln_gl::gpu", "bridge: general");
        log::warn!(target: "wgpu_hal::vulkan::instance", "bridge: wgpu");
        log::info!(target: "wgpu_core::device", "bridge: wgpu info");
        log::error!(target: "other", "bridge: other");
        let here = std::thread::current().id();
        let records: Vec<_> = probe
            .records()
            .into_iter()
            .filter(|seen| seen.thread == here)
            .map(|seen| (seen.severity, seen.event, seen.code, seen.message))
            .collect();
        assert_eq!(
            records,
            [
                (MLN_LOG_SEVERITY_INFO, MLN_LOG_EVENT_RENDER, -1, "bridge: render".to_owned()),
                (MLN_LOG_SEVERITY_ERROR, MLN_LOG_EVENT_GRAPHICS_BACKEND, -1, "bridge: backend".to_owned()),
                (MLN_LOG_SEVERITY_WARNING, MLN_LOG_EVENT_PARSE_STYLE, -1, "bridge: style".to_owned()),
                (MLN_LOG_SEVERITY_ERROR, MLN_LOG_EVENT_DATABASE, 11, "bridge: database".to_owned()),
                (MLN_LOG_SEVERITY_INFO, MLN_LOG_EVENT_GENERAL, -1, "bridge: general".to_owned()),
                (MLN_LOG_SEVERITY_WARNING, MLN_LOG_EVENT_GRAPHICS_BACKEND, -1, "bridge: wgpu".to_owned()),
                (MLN_LOG_SEVERITY_ERROR, MLN_LOG_EVENT_GENERAL, -1, "bridge: other".to_owned()),
            ]
        );
    }

    #[test]
    fn the_platform_log_names_the_severity_thread_event_and_code() {
        let line = std::thread::Builder::new()
            .name("logger".to_owned())
            .spawn(|| {
                [
                    platform_line(log::Level::Error, "Database", 11, "Can't open"),
                    platform_line(log::Level::Warn, "General", -1, ""),
                ]
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(line, ["[ERROR] {logger}[Database](11): Can't open", "[WARNING] {logger}[General]"]);
    }

    #[test]
    fn every_event_has_its_own_target() {
        for (name, value, platform) in EVENTS {
            assert_eq!(event(&format!("mln_engine::{name}")), (value, platform));
        }
        assert_eq!(event("mln_engine::transform"), (MLN_LOG_EVENT_GENERAL, "General"));
        assert_eq!(event("naga::front"), (MLN_LOG_EVENT_GRAPHICS_BACKEND, "GraphicsBackend"));
    }

    #[test]
    fn a_library_device_logs_the_gpu_it_is_on() {
        let _state = LogState::take();
        assert_eq!(set_async(0), MLN_STATUS_OK);
        let probe = Probe::new("graphics device uses ", false);
        assert_eq!(set(&handler(&probe, 0)), MLN_STATUS_OK);
        let mut device = 0;
        // SAFETY: null options, a local handle and a null diagnostic.
        if unsafe { mln_gpu_device_create(core::ptr::null(), &raw mut device, NULL_DIAGNOSTIC) } != MLN_STATUS_OK {
            eprintln!("skipped: no GPU device");
            return;
        }
        mln_gpu_device_release(device);
        // Other tests' devices log on their own threads.
        let here = std::thread::current().id();
        let records: Vec<_> = probe.records().into_iter().filter(|seen| seen.thread == here).collect();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            (records[0].severity, records[0].event, records[0].code),
            (MLN_LOG_SEVERITY_INFO, MLN_LOG_EVENT_RENDER, -1)
        );
    }
}
