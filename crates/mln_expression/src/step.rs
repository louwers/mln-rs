use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{EvalResult, EvaluationContext, EvaluationError, Expr, Node, Output};
use crate::interpolate::parse_stop_label;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::range::Range;
use crate::stops::{Stops, get_covering_stops};
use crate::type_::Type;

/// `["step", input, output, label, output, ...]`.
#[derive(Clone, PartialEq)]
pub struct Step {
    type_: Type,
    input: Expr,
    stops: Stops,
}

impl Step {
    /// A step over `input`, which is a number.
    #[must_use]
    pub fn new_expr(type_: Type, input: Expr, stops: Stops) -> Expr {
        assert_eq!(input.get_type(), Type::Number);
        Expr::new(Node::Step(Box::new(Step { type_, input, stops })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    #[must_use]
    pub(crate) fn get_input(&self) -> &Expr {
        &self.input
    }

    #[must_use]
    pub fn get_covering_stops(&self, lower: f64, upper: f64) -> Range<f32> {
        get_covering_stops(&self.stops, lower, upper)
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();

        if length - 1 < 4 {
            ctx.error(format!("Expected at least 4 arguments, but found only {}.", length - 1));
            return None;
        }

        // [step, input, firstOutput_value, 2 * (n pairs)...]
        if !(length - 1).is_multiple_of(2) {
            ctx.error("Expected an even number of arguments.".to_owned());
            return None;
        }

        let input = ctx.parse(&value.array_member(1), 1, Some(Type::Number), None)?;

        let mut stops = Stops::new();
        let mut output_type: Option<Type> = None;
        if let Some(expected) = ctx.get_expected()
            && expected != Type::Value
        {
            output_type = Some(expected);
        }

        let mut previous = f64::NEG_INFINITY;

        // consume the first output value, which doesn't have a corresponding
        // input value, before proceeding into the "stops" loop below.
        let first_output = ctx.parse(&value.array_member(2), 2, output_type.clone(), None)?;
        if output_type.is_none() {
            output_type = Some(first_output.get_type());
        }
        stops.emplace(f64::NEG_INFINITY, first_output);

        let mut i = 3;
        while i + 1 < length {
            let Some(label) = parse_stop_label(&value.array_member(i)) else {
                ctx.error_at(
                    r#"Input/output pairs for "step" expressions must be defined using literal numeric values (not computed expressions) for the input values."#
                        .to_owned(),
                    i,
                );
                return None;
            };

            if label <= previous {
                ctx.error_at(
                    r#"Input/output pairs for "step" expressions must be arranged with input values in strictly ascending order."#
                        .to_owned(),
                    i,
                );
                return None;
            }
            previous = label;

            let output = ctx.parse(&value.array_member(i + 1), i + 1, output_type.clone(), None)?;
            if output_type.is_none() {
                output_type = Some(output.get_type());
            }

            stops.emplace(label, output);
            i += 2;
        }

        let output_type = output_type.expect("step output type");

        Some(Step::new_expr(output_type, input, stops))
    }

    /// The output of the last stop at or below the input.
    pub(crate) fn eval<'e, T: Output<'e>>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<T> {
        let x = self.input.eval_num(params)? as f32;
        if x.is_nan() {
            return Err(EvaluationError::new("Input is not a number."));
        }

        if self.stops.is_empty() {
            return Err(EvaluationError::new("No stops in step curve."));
        }

        let it = self.stops.upper_bound(f64::from(x));
        T::eval(self.stops.output(it.saturating_sub(1)), params)
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.input);
        self.each_output(visit);
    }

    pub(crate) fn each_output<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        self.stops.values().for_each(visit);
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String("step".to_owned()), self.input.serialize()];
        for (i, (k, v)) in self.stops.iter().enumerate() {
            // Port-only: C++ writes every label but -infinity's. The legacy
            // property functions make steps whose first label is finite
            // (zoom-and-property categorical functions), and
            // `["step", input, label, output, ...]` doesn't parse back. An
            // input below the first label takes the first output anyway, so
            // the first output is written as the step's base output.
            if i > 0 {
                serialized.push(MbglValue::Double(k));
            }
            serialized.push(v.serialize());
        }
        // Port-only: an interval function with a single stop converts to a
        // step without stops, and `["step", input, output]` doesn't parse
        // back ("step" takes at least one stop). Add a stop with the same
        // output, as gl-js's `fixupDegenerateStepCurve` does when it
        // converts such a function.
        if serialized.len() == 3 {
            let output = serialized[2].clone();
            serialized.push(MbglValue::Double(0.0));
            serialized.push(output);
        }
        MbglValue::Array(serialized)
    }
}
