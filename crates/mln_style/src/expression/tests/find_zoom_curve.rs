use crate::expression::dsl::*;
use crate::expression::value::Value;
use crate::expression::zoom_curve::find_zoom_curve_checked;
use crate::log_target;
use crate::tests::util::FixtureLog;

#[test]
fn invalid_zoom_curve_is_logged() {
    let log = FixtureLog::new();

    // A zoom step nested below the top level.
    let expression =
        eq(step(zoom(), literal(Value::Number(1.0)), 5.0, literal(Value::Number(2.0))), literal(Value::Number(1.0)));
    assert!(find_zoom_curve_checked(&expression).is_null());

    assert_eq!(
        log.messages().into_iter().map(|m| (m.level, m.target, m.msg)).collect::<Vec<_>>(),
        [(
            log::Level::Error,
            log_target::STYLE.to_owned(),
            r#"Invalid Expression: "zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#
                .to_owned()
        )]
    );
}
