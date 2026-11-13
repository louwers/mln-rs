use std::sync::mpsc::{Sender, channel};

use mln_util::logging::{Event, EventSeverity, Log, Observer};

use crate::dsl::*;
use crate::find_zoom_curve::find_zoom_curve_checked;
use crate::value::Value;

struct Recorder(Sender<(EventSeverity, Event, String)>);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        let _ = self.0.send((severity, event, msg.to_owned()));
        true
    }
}

#[test]
fn invalid_zoom_curve_is_logged() {
    let (sender, records) = channel();
    let previous = Log::set_thread_observer(Some(Box::new(Recorder(sender))));

    // A zoom step nested below the top level.
    let expression =
        eq(step(zoom(), literal(Value::Number(1.0)), 5.0, literal(Value::Number(2.0))), literal(Value::Number(1.0)));
    assert!(find_zoom_curve_checked(expression.as_ref()).is_null());

    let _ = Log::set_thread_observer(previous);
    assert_eq!(
        records.try_iter().collect::<Vec<_>>(),
        [(
            EventSeverity::Error,
            Event::Style,
            r#"Invalid Expression: "zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#
                .to_owned()
        )]
    );
}
