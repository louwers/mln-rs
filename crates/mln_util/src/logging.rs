// Ports include/mln/util/event.hpp + src/mln/util/event.cpp,
// include/mln/util/logging.hpp + src/mln/util/logging.cpp and
// platform/default/src/mln/util/logging_stderr.cpp (synchronous recording only; the log thread of
// `useLogThread` is a TODO(port)).
//
// Rust addition: `Log::set_thread_observer` installs an observer for the
// current thread only, taking precedence over the global one. The Rust test
// harness runs tests in parallel threads of one process, so the C++
// `FixtureLog` pattern (a process-wide observer) needs per-thread capture.

use std::cell::RefCell;
use std::sync::RwLock;

use crate::sync::RwLockExt;

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

// src/mln/util/event.cpp
crate::define_enum!(
    EventSeverity,
    [
        (EventSeverity::Debug, "DEBUG"),
        (EventSeverity::Info, "INFO"),
        (EventSeverity::Warning, "WARNING"),
        (EventSeverity::Error, "ERROR"),
    ]
);

crate::define_enum!(
    Event,
    [
        (Event::General, "General"),
        (Event::Setup, "Setup"),
        (Event::Shader, "Shader"),
        (Event::ParseStyle, "ParseStyle"),
        (Event::ParseTile, "ParseTile"),
        (Event::Render, "Render"),
        (Event::Style, "Style"),
        (Event::Database, "Database"),
        (Event::HttpRequest, "HttpRequest"),
        (Event::Sprite, "Sprite"),
        (Event::Image, "Image"),
        (Event::GraphicsBackend, "GraphicsBackend"),
        (Event::JNI, "JNI"),
        (Event::Android, "Android"),
        (Event::Crash, "Crash"),
        (Event::Glyph, "Glyph"),
        (Event::Timing, "Timing"),
    ]
);

/// `Log::Observer`.
pub trait Observer: Send + Sync {
    /// Returns true when the record was handled (then it is not printed).
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool;
}

static OBSERVER: RwLock<Option<Box<dyn Observer>>> = RwLock::new(None);

thread_local! {
    static THREAD_OBSERVER: RefCell<Option<Box<dyn Observer>>> = const { RefCell::new(None) };
}

/// `mln::Log`.
pub struct Log;

impl Log {
    pub fn set_observer(observer: Box<dyn Observer>) {
        *OBSERVER.write_unpoisoned() = Some(observer);
    }

    pub fn remove_observer() -> Option<Box<dyn Observer>> {
        OBSERVER.write_unpoisoned().take()
    }

    /// Rust addition (see the file comment): an observer for the calling
    /// thread only. Returns the previous one.
    pub fn set_thread_observer(observer: Option<Box<dyn Observer>>) -> Option<Box<dyn Observer>> {
        THREAD_OBSERVER.with(|o| std::mem::replace(&mut *o.borrow_mut(), observer))
    }

    pub fn debug(event: Event, msg: impl AsRef<str>) {
        Self::record(EventSeverity::Debug, event, -1, msg.as_ref());
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

    /// `Log::Record` with the event filters from event.hpp
    /// (`disabledEventPermutations = {{Debug, Shader}}`; Debug is disabled in
    /// release builds).
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
                None => match &*OBSERVER.read_unpoisoned() {
                    Some(observer) => observer.on_record(severity, event, code, msg),
                    None => false,
                },
            };
            if handled {
                return;
            }
        }

        use crate::enum_::Enum;
        let mut log_stream = String::new();
        log_stream.push('{');
        log_stream.push_str(std::thread::current().name().unwrap_or(""));
        log_stream.push('}');
        log_stream.push('[');
        log_stream.push_str(event.to_string());
        log_stream.push(']');
        if code >= 0 {
            log_stream.push_str(&format!("({code})"));
        }
        if !msg.is_empty() {
            log_stream.push_str(": ");
            log_stream.push_str(msg);
        }
        Self::platform_record(severity, &log_stream);
    }

    /// platform/default/src/mln/util/logging_stderr.cpp.
    fn platform_record(severity: EventSeverity, msg: &str) {
        use crate::enum_::Enum;
        eprintln!("[{}] {}", severity.to_string(), msg);
    }
}
