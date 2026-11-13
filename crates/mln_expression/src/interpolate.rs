// C++ `InterpolateImpl<T>` (a template over the output type) is the
// `InterpolateOutput` tag here; there is one `Interpolate` struct.

use feature::Value as MbglValue;
use mln_conversion::Convertible;
use mln_style_types::VariableAnchorOffsetCollection;
use mln_util::{Color, ColorSpace, Padding};

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::interpolator::{CubicBezierInterpolator, ExponentialInterpolator, Interpolator};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::range::Range;
use crate::stops::{Stops, get_covering_stops};
use crate::type_::{self, Type};
use crate::value::{Value, type_of};

/// The `T` of `InterpolateImpl<T>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterpolateOutput {
    Number,
    Color,
    Padding,
    ProjectionDefinition,
    VariableAnchorOffsetCollection,
    Array,
}

impl InterpolateOutput {
    fn expression_type(self) -> Type {
        match self {
            InterpolateOutput::Number => Type::Number,
            InterpolateOutput::Color => Type::Color,
            InterpolateOutput::Padding => Type::Padding,
            InterpolateOutput::ProjectionDefinition => Type::ProjectionDefinition,
            InterpolateOutput::VariableAnchorOffsetCollection => Type::VariableAnchorOffsetCollection,
            InterpolateOutput::Array => Type::array(Type::Value),
        }
    }

    fn holds(self, v: &Value) -> bool {
        matches!(
            (self, v),
            (InterpolateOutput::Number, Value::Number(_))
                | (InterpolateOutput::Color, Value::Color(_))
                | (InterpolateOutput::Padding, Value::Padding(_))
                | (InterpolateOutput::ProjectionDefinition, Value::ProjectionDefinition(_))
                | (InterpolateOutput::VariableAnchorOffsetCollection, Value::VariableAnchorOffsetCollection(_))
                | (InterpolateOutput::Array, Value::Array(_))
        )
    }
}

fn interpolate_float(a: f32, b: f32, t: f64) -> f32 {
    (f64::from(a) * (1.0 - t) + f64::from(b) * t) as f32
}

/// The generic `Interpolator<double>`.
fn interpolate_double(a: f64, b: f64, t: f64) -> f64 {
    a * (1.0 - t) + b * t
}

pub struct Interpolate {
    base: ExpressionBase,
    interpolator: Interpolator,
    color_space: ColorSpace,
    input: Box<dyn Expression>,
    stops: Stops,
    output: InterpolateOutput,
}

impl Interpolate {
    fn new(
        type_: Type,
        interpolator: Interpolator,
        color_space: ColorSpace,
        input: Box<dyn Expression>,
        stops: Stops,
        output: InterpolateOutput,
    ) -> Self {
        assert_eq!(*input.get_type(), Type::Number);
        let deps = input.dependencies() | stops.values().fold(Dependency::NONE, |a, e| a | e.dependencies());
        Interpolate {
            base: ExpressionBase::new(Kind::Interpolate, type_, deps),
            interpolator,
            color_space,
            input,
            stops,
            output,
        }
    }

    #[must_use]
    pub fn get_input(&self) -> &dyn Expression {
        &*self.input
    }

    #[must_use]
    pub fn get_interpolator(&self) -> &Interpolator {
        &self.interpolator
    }

    #[must_use]
    pub fn get_color_space(&self) -> ColorSpace {
        self.color_space
    }

    #[must_use]
    pub fn get_stop_count(&self) -> usize {
        self.stops.len()
    }

    pub fn each_stop(&self, mut visit: impl FnMut(f64, &dyn Expression)) {
        for (k, v) in self.stops.iter() {
            visit(k, v);
        }
    }

    /// Return the smallest range of stops that covers the interval
    /// [lower, upper].
    #[must_use]
    pub fn get_covering_stops(&self, lower: f64, upper: f64) -> Range<f32> {
        get_covering_stops(&self.stops, lower, upper)
    }

    #[must_use]
    pub fn interpolation_factor(&self, input_levels: Range<f64>, input_value: f64) -> f64 {
        self.interpolator.interpolation_factor(input_levels, input_value)
    }

    fn interpolate_outputs(&self, lower: &Value, upper: &Value, t: f64) -> EvaluationResult {
        Ok(match (lower, upper) {
            (Value::Number(a), Value::Number(b)) => Value::Number(interpolate_double(*a, *b, t)),
            (Value::Color(a), Value::Color(b)) => Value::Color(Color::interpolate(a, b, t, self.color_space)),
            (Value::Padding(a), Value::Padding(b)) => Value::Padding(Padding::new(
                interpolate_float(a.top, b.top, t),
                interpolate_float(a.right, b.right, t),
                interpolate_float(a.bottom, b.bottom, t),
                interpolate_float(a.left, b.left, t),
            )),
            (Value::ProjectionDefinition(a), Value::ProjectionDefinition(b)) => {
                Value::ProjectionDefinition(mln_util::interpolate::interpolate(a, b, t))
            }
            (Value::VariableAnchorOffsetCollection(a), Value::VariableAnchorOffsetCollection(b)) => {
                // Interpolator<VariableAnchorOffsetCollection> only has a
                // float overload. It throws std::runtime_error on mismatched
                // collections, which propagates out of evaluate() in C++.
                // Here it is an evaluation error.
                match VariableAnchorOffsetCollection::interpolate(a, b, t as f32) {
                    Ok(v) => Value::VariableAnchorOffsetCollection(v),
                    Err(e) => return Err(EvaluationError::new(e)),
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                // Interpolator<std::vector<Value>>
                debug_assert_eq!(a.len(), b.len());
                if a.is_empty() {
                    return Ok(Value::Array(Vec::new()));
                }
                let mut result = Vec::with_capacity(a.len());
                for (ai, bi) in a.iter().zip(b) {
                    let ai = ai.as_number().expect("numeric array item");
                    let bi = bi.as_number().expect("numeric array item");
                    result.push(Value::Number(interpolate_double(ai, bi, t)));
                }
                Value::Array(result)
            }
            _ => unreachable!("interpolate_outputs on mismatched types"),
        })
    }
}

impl Expression for Interpolate {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_input = self.input.evaluate(params)?;

        let x: f32 = evaluated_input.as_number().map(|d| d as f32).expect("numeric interpolation input");
        if x.is_nan() {
            return Err(EvaluationError::new("Input is not a number."));
        }

        if self.stops.is_empty() {
            return Err(EvaluationError::new("No stops in exponential curve."));
        }

        let it = self.stops.upper_bound(f64::from(x));
        if it == self.stops.len() {
            self.stops.get(self.stops.len() - 1).1.evaluate(params)
        } else if it == 0 {
            self.stops.get(0).1.evaluate(params)
        } else {
            let (lower_key, lower_expr) = self.stops.get(it - 1);
            let (upper_key, upper_expr) = self.stops.get(it);
            let t = self.interpolation_factor(Range::new(lower_key, upper_key), f64::from(x));

            if t == 0.0 {
                return lower_expr.evaluate(params);
            }
            if t == 1.0 {
                return upper_expr.evaluate(params);
            }

            let lower = lower_expr.evaluate(params)?;
            let upper = upper_expr.evaluate(params)?;

            if !self.output.holds(&lower) {
                return Err(EvaluationError::new(format!(
                    "Expected value to be of type {}, but found {} instead.",
                    type_::to_string(&self.output.expression_type()),
                    type_::to_string(&type_of(&lower))
                )));
            }

            if !self.output.holds(&upper) {
                return Err(EvaluationError::new(format!(
                    "Expected value to be of type {}, but found {} instead.",
                    type_::to_string(&self.output.expression_type()),
                    type_::to_string(&type_of(&upper))
                )));
            }
            self.interpolate_outputs(&lower, &upper, t)
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.input);
        for v in self.stops.values() {
            visit(&**v);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Interpolate {
            let rhs = downcast::<Interpolate>(e).expect("Interpolate");
            if self.interpolator != rhs.interpolator
                || self.color_space != rhs.color_space
                || !self.input.equals(&*rhs.input)
                || self.stops.len() != rhs.stops.len()
            {
                return false;
            }

            return self.stops.children_equal(&rhs.stops);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for stop in self.stops.values() {
            result.extend(stop.possible_outputs());
        }
        result
    }

    fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator())];

        match &self.interpolator {
            Interpolator::Exponential(exponential) => {
                if exponential.base == 1.0 {
                    serialized.push(MbglValue::Array(vec![MbglValue::String("linear".to_owned())]));
                } else {
                    serialized.push(MbglValue::Array(vec![
                        MbglValue::String("exponential".to_owned()),
                        MbglValue::Double(exponential.base),
                    ]));
                }
            }
            Interpolator::CubicBezier(cubic_bezier) => {
                // `ub.getP1()`, `ub.getP2()` in C++ (see `control_points`).
                let [x1, y1, x2, y2] = cubic_bezier.control_points();
                serialized.push(MbglValue::Array(vec![
                    MbglValue::String("cubic-bezier".to_owned()),
                    MbglValue::Double(x1),
                    MbglValue::Double(y1),
                    MbglValue::Double(x2),
                    MbglValue::Double(y2),
                ]));
            }
        }
        serialized.push(self.input.serialize());
        for (k, v) in self.stops.iter() {
            serialized.push(MbglValue::Double(k));
            serialized.push(v.serialize());
        }
        MbglValue::Array(serialized)
    }

    fn get_operator(&self) -> String {
        match self.color_space {
            ColorSpace::Lab => "interpolate-lab",
            ColorSpace::Hcl => "interpolate-hcl",
            ColorSpace::Oklch => "interpolate-oklch",
            ColorSpace::Rgb => "interpolate",
        }
        .to_owned()
    }
}

/// The label of an interpolate/step stop (`toValue` of a JSON number).
pub(crate) fn parse_stop_label(value: &Convertible<'_>) -> Option<f64> {
    match value.to_value()? {
        MbglValue::Uint(n) => {
            let n = n as f64;
            Some(if n > f64::MAX { f64::INFINITY } else { n })
        }
        MbglValue::Int(n) => {
            let n = n as f64;
            Some(if n > f64::MAX { f64::INFINITY } else { n })
        }
        MbglValue::Double(n) => Some(if n > f64::MAX { f64::INFINITY } else { n }),
        _ => None,
    }
}

pub fn parse_interpolate(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    debug_assert!(value.is_array());

    let length = value.array_length();

    if length < 2 {
        ctx.error("Expected an interpolation type expression.".to_owned());
        return None;
    }

    let interp = value.array_member(1);
    if !interp.is_array() || interp.array_length() == 0 {
        ctx.error("Expected an interpolation type expression.".to_owned());
        return None;
    }

    let op = value.array_member(0).to_string();
    let color_space = match op.as_deref() {
        Some("interpolate-lab") => ColorSpace::Lab,
        Some("interpolate-hcl") => ColorSpace::Hcl,
        Some("interpolate-oklch") => ColorSpace::Oklch,
        _ => ColorSpace::Rgb,
    };

    let mut interpolator: Option<Interpolator> = None;

    let interp_name = interp.array_member(0).to_string();
    if interp_name.as_deref() == Some("linear") {
        interpolator = Some(ExponentialInterpolator::new(1.0).into());
    } else if interp_name.as_deref() == Some("exponential") {
        let mut base = None;
        if interp.array_length() == 2 {
            base = interp.array_member(1).to_double();
        }
        let Some(base) = base else {
            ctx.error_at2("Exponential interpolation requires a numeric base.".to_owned(), 1, 1);
            return None;
        };
        interpolator = Some(ExponentialInterpolator::new(base).into());
    } else if interp_name.as_deref() == Some("cubic-bezier") {
        let (mut x1, mut y1, mut x2, mut y2) = (None, None, None, None);
        if interp.array_length() == 5 {
            x1 = interp.array_member(1).to_double();
            y1 = interp.array_member(2).to_double();
            x2 = interp.array_member(3).to_double();
            y2 = interp.array_member(4).to_double();
        }
        let in_unit = |v: Option<f64>| v.is_some_and(|v| !(v < 0.0 || v > 1.0));
        if !in_unit(x1) || !in_unit(y1) || !in_unit(x2) || !in_unit(y2) {
            ctx.error_at(
                "Cubic bezier interpolation requires four numeric arguments with values between 0 and 1.".to_owned(),
                1,
            );
            return None;
        }
        interpolator = Some(CubicBezierInterpolator::new(x1.unwrap(), y1.unwrap(), x2.unwrap(), y2.unwrap()).into());
    }

    let Some(interpolator) = interpolator else {
        ctx.error_at2(format!("Unknown interpolation type {}", interp_name.as_deref().unwrap_or("")), 1, 0);
        return None;
    };

    let min_args = 4;
    if length - 1 < min_args {
        ctx.error(format!("Expected at least 4 arguments, but found only {}.", length - 1));
        return None;
    }

    // [interpolation, interp_type, input, 2 * (n pairs)...]
    if !(length - 1).is_multiple_of(2) {
        ctx.error("Expected an even number of arguments.".to_owned());
        return None;
    }

    let input = ctx.parse(&value.array_member(2), 2, Some(Type::Number), None)?;

    let mut stops = Stops::new();
    let mut output_type: Option<Type> = None;
    if color_space != ColorSpace::Rgb {
        output_type = Some(Type::Color);
    } else if let Some(expected) = ctx.get_expected()
        && expected != Type::Value
    {
        output_type = Some(expected);
    }

    let mut previous = f64::NEG_INFINITY;

    let mut i = 3;
    while i + 1 < length {
        let Some(label) = parse_stop_label(&value.array_member(i)) else {
            ctx.error_at(
                r#"Input/output pairs for "interpolate" expressions must be defined using literal numeric values (not computed expressions) for the input values."#
                    .to_owned(),
                i,
            );
            return None;
        };

        if label <= previous {
            ctx.error_at(
                r#"Input/output pairs for "interpolate" expressions must be arranged with input values in strictly ascending order."#
                    .to_owned(),
                i,
            );
            return None;
        }
        previous = label;

        let output = ctx.parse(&value.array_member(i + 1), i + 1, output_type.clone(), None)?;
        if output_type.is_none() {
            output_type = Some(output.get_type().clone());
        }

        stops.emplace(label, output);
        i += 2;
    }

    let output_type = output_type.expect("interpolate output type");

    create_interpolate(output_type, interpolator, color_space, input, stops, ctx)
}

pub fn create_interpolate(
    type_: Type,
    interpolator: Interpolator,
    color_space: ColorSpace,
    input: Box<dyn Expression>,
    stops: Stops,
    ctx: &mut ParsingContext,
) -> ParseResult {
    debug_assert!(color_space == ColorSpace::Rgb || type_ == Type::Color);
    let output = match &type_ {
        Type::Number => InterpolateOutput::Number,
        Type::Color => InterpolateOutput::Color,
        Type::Padding => InterpolateOutput::Padding,
        Type::ProjectionDefinition => InterpolateOutput::ProjectionDefinition,
        Type::VariableAnchorOffsetCollection => InterpolateOutput::VariableAnchorOffsetCollection,
        Type::Array(array_type) => {
            if array_type.item_type != Type::Number || array_type.n.is_none() {
                ctx.error(format!("Type {} is not interpolatable.", type_::to_string(&type_)));
                return None;
            }
            InterpolateOutput::Array
        }
        _ => {
            ctx.error(format!("Type {} is not interpolatable.", type_::to_string(&type_)));
            return None;
        }
    };
    Some(Box::new(Interpolate::new(type_, interpolator, color_space, input, stops, output)))
}
