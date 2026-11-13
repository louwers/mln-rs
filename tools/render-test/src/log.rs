#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    General,
    ParseStyle,
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::General => "General",
            Event::ParseStyle => "ParseStyle",
        }
    }
}

fn record(severity: &str, event: Event, msg: &str) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unknown");
    if msg.is_empty() {
        eprintln!("[{severity}] {{{name}}}[{}]", event.name());
    } else {
        eprintln!("[{severity}] {{{name}}}[{}]: {msg}", event.name());
    }
}

pub fn info(event: Event, msg: &str) {
    record("INFO", event, msg);
}

pub fn warning(event: Event, msg: &str) {
    record("WARNING", event, msg);
}

pub fn error(event: Event, msg: &str) {
    record("ERROR", event, msg);
}
