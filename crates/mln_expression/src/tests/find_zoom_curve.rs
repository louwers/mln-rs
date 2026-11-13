use std::sync::{Arc, Mutex};

use mln_util::logging::{Event, EventSeverity, Log, Observer};

use crate::dsl::*;
use crate::find_zoom_curve::find_zoom_curve_checked;
use crate::value::Value;

type Records = Arc<Mutex<Vec<(EventSeverity, Event, String)>>>;

struct Recorder(Records);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        self.0.lock().unwrap().push((severity, event, msg.to_owned()));
        true
    }
}

#[test]
fn invalid_zoom_curve_is_logged() {
    let records: Records = Arc::default();
    let previous = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&records)))));

    // A zoom step nested below the top level.
    let expression =
        eq(step(zoom(), literal(Value::Number(1.0)), 5.0, literal(Value::Number(2.0))), literal(Value::Number(1.0)));
    assert!(find_zoom_curve_checked(expression.as_ref()).is_null());

    let _ = Log::set_thread_observer(previous);
    assert_eq!(
        *records.lock().unwrap(),
        [(
            EventSeverity::Error,
            Event::Style,
            r#"Invalid Expression: "zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#
                .to_owned()
        )]
    );
}
