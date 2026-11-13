use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::interpolate::parse_stop_label;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::range::Range;
use crate::stops::{Stops, get_covering_stops};
use crate::type_::Type;
use crate::value::Value;

pub struct Step {
    base: ExpressionBase,
    input: Box<dyn Expression>,
    stops: Stops,
}

impl Step {
    #[must_use]
    pub fn new(type_: Type, input: Box<dyn Expression>, stops: Stops) -> Self {
        assert_eq!(*input.get_type(), Type::Number);
        let deps = input.dependencies() | stops.values().fold(Dependency::NONE, |a, e| a | e.dependencies());
        Step { base: ExpressionBase::new(Kind::Step, type_, deps), input, stops }
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

    #[must_use]
    pub fn get_input(&self) -> &dyn Expression {
        &*self.input
    }

    #[must_use]
    pub fn get_covering_stops(&self, lower: f64, upper: f64) -> Range<f32> {
        get_covering_stops(&self.stops, lower, upper)
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
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
            output_type = Some(first_output.get_type().clone());
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
                output_type = Some(output.get_type().clone());
            }

            stops.emplace(label, output);
            i += 2;
        }

        let output_type = output_type.expect("step output type");

        Some(Box::new(Step::new(output_type, input, stops)))
    }
}

impl Expression for Step {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_input = self.input.evaluate(params)?;

        let x: f32 = evaluated_input.as_number().map(|d| d as f32).expect("numeric step input");
        if x.is_nan() {
            return Err(EvaluationError::new("Input is not a number."));
        }

        if self.stops.is_empty() {
            return Err(EvaluationError::new("No stops in step curve."));
        }

        let it = self.stops.upper_bound(f64::from(x));
        if it == self.stops.len() {
            self.stops.get(self.stops.len() - 1).1.evaluate(params)
        } else if it == 0 {
            self.stops.get(0).1.evaluate(params)
        } else {
            self.stops.get(it - 1).1.evaluate(params)
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.input);
        for v in self.stops.values() {
            visit(&**v);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Step {
            let rhs = downcast::<Step>(e).expect("Step");
            return self.input.equals(&*rhs.input) && self.stops.children_equal(&rhs.stops);
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
        let mut serialized = vec![MbglValue::String(self.get_operator()), self.input.serialize()];
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

    fn get_operator(&self) -> String {
        "step".to_owned()
    }
}
