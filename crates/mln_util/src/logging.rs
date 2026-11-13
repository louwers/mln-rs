// Rust addition: `Log::set_thread_observer` installs an observer for the
// current thread only, taking precedence over the global one. The Rust test
// harness runs tests in parallel threads of one process, so the C++
// `FixtureLog` pattern (a process-wide observer) needs per-thread capture.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::str::FromStr;

use parking_lot::RwLock;

use crate::ParseEnumError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EventSeverity {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Event {
    General,
    Setup,
    Shader,
    ParseStyle,
    ParseTile,
    Render,
    Style,
    Database,
    HttpRequest,
    Sprite,
    Image,
    GraphicsBackend,
    JNI,
    Android,
    Crash,
    Glyph,
    Timing,
}

impl EventSeverity {
    /// The name log lines print.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            EventSeverity::Debug => "DEBUG",
            EventSeverity::Info => "INFO",
            EventSeverity::Warning => "WARNING",
            EventSeverity::Error => "ERROR",
        }
    }
}

impl FromStr for EventSeverity {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DEBUG" => Ok(EventSeverity::Debug),
            "INFO" => Ok(EventSeverity::Info),
            "WARNING" => Ok(EventSeverity::Warning),
            "ERROR" => Ok(EventSeverity::Error),
            _ => Err(ParseEnumError),
        }
    }
}

impl Event {
    /// The name log lines print.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Event::General => "General",
            Event::Setup => "Setup",
            Event::Shader => "Shader",
            Event::ParseStyle => "ParseStyle",
            Event::ParseTile => "ParseTile",
            Event::Render => "Render",
            Event::Style => "Style",
            Event::Database => "Database",
            Event::HttpRequest => "HttpRequest",
            Event::Sprite => "Sprite",
            Event::Image => "Image",
            Event::GraphicsBackend => "GraphicsBackend",
            Event::JNI => "JNI",
            Event::Android => "Android",
            Event::Crash => "Crash",
            Event::Glyph => "Glyph",
            Event::Timing => "Timing",
        }
    }
}

impl FromStr for Event {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "General" => Ok(Event::General),
            "Setup" => Ok(Event::Setup),
            "Shader" => Ok(Event::Shader),
            "ParseStyle" => Ok(Event::ParseStyle),
            "ParseTile" => Ok(Event::ParseTile),
            "Render" => Ok(Event::Render),
            "Style" => Ok(Event::Style),
            "Database" => Ok(Event::Database),
            "HttpRequest" => Ok(Event::HttpRequest),
            "Sprite" => Ok(Event::Sprite),
            "Image" => Ok(Event::Image),
            "GraphicsBackend" => Ok(Event::GraphicsBackend),
            "JNI" => Ok(Event::JNI),
            "Android" => Ok(Event::Android),
            "Crash" => Ok(Event::Crash),
            "Glyph" => Ok(Event::Glyph),
            "Timing" => Ok(Event::Timing),
            _ => Err(ParseEnumError),
        }
    }
}

pub trait Observer: Send + Sync {
    /// Returns true when the record was handled (then it is not printed).
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool;
}

static OBSERVER: RwLock<Option<Box<dyn Observer>>> = RwLock::new(None);

thread_local! {
    static THREAD_OBSERVER: RefCell<Option<Box<dyn Observer>>> = const { RefCell::new(None) };
}

pub struct Log;

impl Log {
    pub fn set_observer(observer: Box<dyn Observer>) {
        *OBSERVER.write() = Some(observer);
    }

    pub fn remove_observer() -> Option<Box<dyn Observer>> {
        OBSERVER.write().take()
    }

    /// Rust addition (see the file comment): an observer for the calling
    /// thread only. Returns the previous one.
    #[must_use]
    pub fn set_thread_observer(observer: Option<Box<dyn Observer>>) -> Option<Box<dyn Observer>> {
        THREAD_OBSERVER.with(|o| std::mem::replace(&mut *o.borrow_mut(), observer))
    }

    pub fn info(event: Event, msg: impl AsRef<str>) {
        Self::record(EventSeverity::Info, event, -1, msg.as_ref());
    }

    pub fn warning(event: Event, msg: impl AsRef<str>) {
        Self::record(EventSeverity::Warning, event, -1, msg.as_ref());
    }

    pub fn error(event: Event, msg: impl AsRef<str>) {
        Self::record(EventSeverity::Error, event, -1, msg.as_ref());
    }

    /// Records a log message, unless it is filtered out: `Debug` severity is
    /// dropped for `Shader` events and in release builds.
    pub fn record(severity: EventSeverity, event: Event, code: i64, msg: &str) {
        if !cfg!(debug_assertions) && severity == EventSeverity::Debug {
            return;
        }
        if severity == EventSeverity::Debug && event == Event::Shader {
            return;
        }

        // C++ never hands Debug records to the observer.
        if severity != EventSeverity::Debug {
            let handled = THREAD_OBSERVER
                .with(|o| o.borrow().as_ref().map(|observer| observer.on_record(severity, event, code, msg)));
            let handled = match handled {
                Some(handled) => handled,
                None => match &*OBSERVER.read() {
                    Some(observer) => observer.on_record(severity, event, code, msg),
                    None => false,
                },
            };
            if handled {
                return;
            }
        }

        let mut log_stream = String::new();
        log_stream.push('{');
        log_stream.push_str(std::thread::current().name().unwrap_or(""));
        log_stream.push('}');
        log_stream.push('[');
        log_stream.push_str(event.as_str());
        log_stream.push(']');
        if code >= 0 {
            let _ = write!(log_stream, "({code})");
        }
        if !msg.is_empty() {
            log_stream.push_str(": ");
            log_stream.push_str(msg);
        }
        Self::platform_record(severity, &log_stream);
    }

    /// `platform/default/src/mln/util/logging_stderr.cpp`.
    fn platform_record(severity: EventSeverity, msg: &str) {
        eprintln!("[{}] {}", severity.as_str(), msg);
    }
}
