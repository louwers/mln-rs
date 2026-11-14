use std::sync::mpsc::{Sender, channel};

use mln_util::logging::{Event, EventSeverity, Log, Observer};

use crate::Device;

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
    let device = Device::new(None).expect("Vulkan device");
    drop(Log::set_thread_observer(previous));

    let name = device.shared.properties().device_name_as_c_str().unwrap().to_string_lossy().into_owned();
    // The validation layer's warnings aside.
    let records: Vec<_> = records.try_iter().filter(|(severity, ..)| *severity == EventSeverity::Info).collect();
    assert_eq!(
        records,
        [(EventSeverity::Info, Event::Render, -1, format!("graphics device uses Vulkan physical device {name}"))]
    );
}
