use crate::coalesce::Coalesce;
use crate::compound_expression::CompoundExpression;
use crate::expression::{Dependency, Expression, Kind, downcast};
use crate::interpolate::Interpolate;
use crate::let_::Let;
use crate::parsing_context::ParsingError;
use crate::step::Step;

/// The alternatives of `variant<const Interpolate*, const Step*, ParsingError>`.
#[derive(Clone)]
pub enum ZoomCurve<'a> {
    Interpolate(&'a Interpolate),
    Step(&'a Step),
    Error(ParsingError),
}

impl PartialEq for ZoomCurve<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ZoomCurve::Interpolate(a), ZoomCurve::Interpolate(b)) => std::ptr::eq(*a, *b),
            (ZoomCurve::Step(a), ZoomCurve::Step(b)) => std::ptr::eq(*a, *b),
            (ZoomCurve::Error(a), ZoomCurve::Error(b)) => a == b,
            _ => false,
        }
    }
}

impl std::fmt::Debug for ZoomCurve<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZoomCurve::Interpolate(_) => f.write_str("Interpolate"),
            ZoomCurve::Step(_) => f.write_str("Step"),
            ZoomCurve::Error(e) => write!(f, "Error({e:?})"),
        }
    }
}

pub type ZoomCurveOrError<'a> = Option<ZoomCurve<'a>>;

/// The zoom curve of an expression: none, an interpolate or a step.
#[derive(Clone, Copy)]
pub enum ZoomCurvePtr<'a> {
    Null,
    Interpolate(&'a Interpolate),
    Step(&'a Step),
}

impl ZoomCurvePtr<'_> {
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, ZoomCurvePtr::Null)
    }
}

fn zoom_input(input: &dyn Expression) -> bool {
    input.get_kind() == Kind::CompoundExpression
        && downcast::<CompoundExpression>(input).is_some_and(|z| z.get_operator() == "zoom")
}

/// Find a zoom curve in the expression tree: the relevant step or
/// interpolate item, or the error if the expression represents an error.
pub fn find_zoom_curve(e: &dyn Expression) -> ZoomCurveOrError<'_> {
    let mut result: ZoomCurveOrError<'_> = None;

    match e.get_kind() {
        Kind::Let => {
            let let_ = downcast::<Let>(e).expect("Let");
            result = find_zoom_curve(let_.get_result());
        }
        Kind::Coalesce => {
            let coalesce = downcast::<Coalesce>(e).expect("Coalesce");
            let length = coalesce.get_length();
            for i in 0..length {
                let child = coalesce.get_child(i);
                result = find_zoom_curve(child);
                if result.is_some() {
                    break;
                }
            }
        }
        Kind::Interpolate => {
            let curve = downcast::<Interpolate>(e).expect("Interpolate");
            if zoom_input(curve.get_input()) {
                result = Some(ZoomCurve::Interpolate(curve));
            }
        }
        Kind::Step => {
            let step = downcast::<Step>(e).expect("Step");
            if zoom_input(step.get_input()) {
                result = Some(ZoomCurve::Step(step));
            }
        }
        _ => {}
    }

    if let Some(ZoomCurve::Error(_)) = result {
        return result;
    }

    e.each_child(&mut |child| {
        let child_result = find_zoom_curve(child);
        if let Some(child_result) = child_result {
            if let ZoomCurve::Error(_) = child_result {
                result = Some(child_result);
            } else if result.is_none() {
                result = Some(ZoomCurve::Error(ParsingError {
                    message: r#""zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#.to_owned(),
                    key: String::new(),
                }));
            } else if result.as_ref() != Some(&child_result) {
                result = Some(ZoomCurve::Error(ParsingError {
                    message: r#"Only one zoom-based "step" or "interpolate" subexpression may be used in an expression."#.to_owned(),
                    key: String::new(),
                }));
            }
        }
    });

    result
}

/// Find a zoom curve in the expression tree: the relevant step or
/// interpolate item, or null if the expression represents an error.
pub fn find_zoom_curve_checked(e: &dyn Expression) -> ZoomCurvePtr<'_> {
    if !e.has(Dependency::ZOOM) {
        return ZoomCurvePtr::Null;
    }
    match find_zoom_curve(e).expect("zoom-dependent expression without a zoom curve") {
        ZoomCurve::Error(err) => {
            // C++ also logs "Invalid Expression: <message>" here.
            let _ = err;
            ZoomCurvePtr::Null
        }
        ZoomCurve::Interpolate(i) => ZoomCurvePtr::Interpolate(i),
        ZoomCurve::Step(s) => ZoomCurvePtr::Step(s),
    }
}
