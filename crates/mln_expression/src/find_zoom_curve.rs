use mln_util::logging::{Event, Log};

use crate::compound_expression::Op;
use crate::expression::{Dependency, Expr, Node};
use crate::interpolate::Interpolate;
use crate::parsing_context::ParsingError;
use crate::step::Step;

/// The alternatives of `variant<const Interpolate*, const Step*, ParsingError>`.
#[derive(Clone)]
pub(crate) enum ZoomCurve<'a> {
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

pub(crate) type ZoomCurveOrError<'a> = Option<ZoomCurve<'a>>;

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

fn zoom_input(input: &Expr) -> bool {
    input.as_compound().is_some_and(|z| z.signature().op == Op::Zoom)
}

/// Find a zoom curve in the expression tree: the relevant step or
/// interpolate item, or the error if the expression represents an error.
pub(crate) fn find_zoom_curve(e: &Expr) -> ZoomCurveOrError<'_> {
    let mut result: ZoomCurveOrError<'_> = None;

    match e.node() {
        Node::Let(let_) => {
            result = find_zoom_curve(let_.result());
        }
        Node::Coalesce(coalesce) => {
            for child in coalesce.args() {
                result = find_zoom_curve(child);
                if result.is_some() {
                    break;
                }
            }
        }
        Node::Interpolate(curve) => {
            if zoom_input(curve.get_input()) {
                result = Some(ZoomCurve::Interpolate(curve));
            }
        }
        Node::Step(step) if zoom_input(step.get_input()) => {
            result = Some(ZoomCurve::Step(step));
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
/// interpolate item, or null if the expression represents an error, which
/// is logged.
#[must_use]
pub fn find_zoom_curve_checked(e: &Expr) -> ZoomCurvePtr<'_> {
    if !e.has(Dependency::ZOOM) {
        return ZoomCurvePtr::Null;
    }
    match find_zoom_curve(e).expect("zoom-dependent expression without a zoom curve") {
        ZoomCurve::Error(err) => {
            Log::error(Event::Style, format!("Invalid Expression: {}", err.message));
            ZoomCurvePtr::Null
        }
        ZoomCurve::Interpolate(i) => ZoomCurvePtr::Interpolate(i),
        ZoomCurve::Step(s) => ZoomCurvePtr::Step(s),
    }
}
