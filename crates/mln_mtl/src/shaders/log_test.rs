#![cfg(target_vendor = "apple")]

// A library device logs the GPU it is on, as maplibre-native-ffi's graphics
// devices do.

use std::sync::mpsc::{Sender, channel};

use mln_mtl::Device;
use mln_util::logging::{Event, EventSeverity, Log, Observer};

struct Recorder(Sender<(EventSeverity, Event, i64, String)>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        let _ = self.0.send((severity, event, code, msg.to_owned()));
        true
    }
}

#[test]
fn a_device_logs_its_gpu() {
    let (sender, records) = channel();
    let previous = Log::set_thread_observer(Some(Box::new(Recorder(sender))));
    let device = Device::new(None).expect("Metal device");
    drop(Log::set_thread_observer(previous));

    let message = format!("graphics device uses Metal device {}", device.name());
    assert_eq!(records.try_iter().collect::<Vec<_>>(), [(EventSeverity::Info, Event::Render, -1, message)]);
}
