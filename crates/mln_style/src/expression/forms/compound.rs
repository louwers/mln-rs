// The compound expressions: calls of the functions (crate::expression::functions) that
// parsing resolves by name. A compound expression keeps the overload its
// arguments matched and evaluates through its evaluator.

use std::borrow::Cow;
use std::sync::Arc;

use crate::color::Color;
use crate::conversion::Convertible;
use crate::expression::functions::{Eval, FunctionDef, Overload, Params, Role, decoding_overload};
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::check_subtype;
use crate::expression::value::Value;
use crate::expression::{EvalResult, EvaluationContext, EvaluationResult, Expr, Node, Output, type_};
use crate::feature::Value as MbglValue;

/// A call of a function with typed arguments.
#[derive(Clone)]
pub(crate) struct CompoundExpression {
    overload: Arc<Overload>,
    args: Box<[Expr]>,
}

/// A compound expression crosses as its function's name, the overload's
/// position among the name's and its arguments. Reading resolves the name
/// against the functions in the reader's context.
impl mln_wire::Wire for CompoundExpression {
    fn write(&self, w: &mut mln_wire::Writer) {
        mln_wire::write_str(w, &self.overload.name);
        self.overload.index.write(w);
        self.args.write(w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        let name = mln_wire::read_str(r)?;
        let index = u8::read(r)?;
        let overload = match decoding_overload(r, name, index) {
            Ok(overload) => overload,
            Err(message) => return r.fail(message),
        };
        let args = Box::<[Expr]>::read(r)?;
        Some(CompoundExpression { overload, args })
    }
}

/// Equal when they call the same overload of the same name with equal
/// arguments, so that a decoded copy equals its original.
impl PartialEq for CompoundExpression {
    fn eq(&self, other: &Self) -> bool {
        self.overload.index == other.overload.index
            && self.overload.name == other.overload.name
            && self.args == other.args
    }
}

impl CompoundExpression {
    /// The overload the arguments matched.
    #[must_use]
    pub(crate) fn overload(&self) -> &Overload {
        &self.overload
    }

    #[must_use]
    pub(crate) fn args(&self) -> &[Expr] {
        &self.args
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.overload.name().to_owned())];
        serialized.extend(self.args.iter().map(Expr::serialize));
        MbglValue::Array(serialized)
    }

    pub(crate) fn eval_num(&self, cx: &EvaluationContext<'_>) -> EvalResult<f64> {
        match self.overload.eval {
            Eval::Number(f) => f(&self.args, cx),
            _ => f64::from_value(self.evaluate(cx)?),
        }
    }

    pub(crate) fn eval_bool(&self, cx: &EvaluationContext<'_>) -> EvalResult<bool> {
        match self.overload.eval {
            Eval::Boolean(f) => f(&self.args, cx),
            _ => bool::from_value(self.evaluate(cx)?),
        }
    }

    pub(crate) fn eval_str<'e>(&'e self, cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
        match self.overload.eval {
            Eval::String(f) => f(&self.args, cx),
            _ => Cow::from_value(self.evaluate(cx)?),
        }
    }

    pub(crate) fn eval_color(&self, cx: &EvaluationContext<'_>) -> EvalResult<Color> {
        match self.overload.eval {
            Eval::Color(f) => f(&self.args, cx),
            _ => Color::from_value(self.evaluate(cx)?),
        }
    }

    pub(crate) fn evaluate(&self, cx: &EvaluationContext<'_>) -> EvaluationResult {
        match self.overload.eval {
            Eval::Number(f) => f(&self.args, cx).map(Value::Number),
            Eval::Boolean(f) => f(&self.args, cx).map(Value::Bool),
            Eval::String(f) => f(&self.args, cx).map(|s| Value::String(s.into_owned())),
            Eval::Color(f) => f(&self.args, cx).map(Value::Color),
            Eval::Value(f) => f(&self.args, cx),
        }
    }
}

fn expected_types_error(overloads: &[Arc<Overload>], args: &[Expr]) -> String {
    // Only used if there are no overloads with matching number of args
    let mut available: Vec<String> = Vec::new();
    let mut matching: Vec<String> = Vec::new();
    for overload in overloads {
        match &overload.params {
            Params::Varargs(t) => {
                matching.push(format!("({})", type_::to_string(t)));
            }
            Params::Fixed(params) => {
                let types = format!("({})", params.iter().map(type_::to_string).collect::<Vec<_>>().join(", "));
                if params.len() == args.len() {
                    matching.push(types);
                } else {
                    available.push(types);
                }
            }
        }
    }

    let signatures = if matching.is_empty() { available.join(" | ") } else { matching.join(" | ") };

    let actual_types = args.iter().map(|arg| type_::to_string(&arg.get_type())).collect::<Vec<_>>().join(", ");

    format!("Expected arguments of type {signatures}, but found ({actual_types}) instead.")
}

fn create_compound_expression_from(
    overloads: &[Arc<Overload>],
    args: Vec<Expr>,
    ctx: &mut ParsingContext,
) -> ParseResult {
    let mut signature_context = ctx.detached();

    for overload in overloads {
        signature_context.clear_errors();

        match &overload.params {
            Params::Fixed(params) => {
                if params.len() != args.len() {
                    signature_context.error(format!(
                        "Expected {} arguments, but found {} instead.",
                        params.len(),
                        args.len()
                    ));
                    continue;
                }

                for (i, arg) in args.iter().enumerate() {
                    if let Some(err) = check_subtype(&params[i], &arg.get_type()) {
                        signature_context.error_at(err, i + 1);
                    }
                }
            }
            Params::Varargs(param_type) => {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(err) = check_subtype(param_type, &arg.get_type()) {
                        signature_context.error_at(err, i + 1);
                    }
                }
            }
        }

        if !signature_context.has_errors() {
            return Some(Expr::new(Node::Compound(Box::new(CompoundExpression {
                overload: Arc::clone(overload),
                args: args.into(),
            }))));
        }
    }

    if overloads.len() == 1 {
        ctx.append_errors(&signature_context);
    } else {
        ctx.error(expected_types_error(overloads, &args));
    }

    None
}

pub(crate) fn parse_compound_expression(name: &str, value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    debug_assert!(value.is_array() && value.array_length() > 0);

    let functions = Arc::clone(ctx.functions());
    let Some(function) = functions.get(name) else {
        ctx.error_at(
            format!(r#"Unknown expression "{name}". If you wanted a literal array, use ["literal", [...]]."#),
            0,
        );
        return None;
    };
    let overloads = function.overloads();

    let length = value.array_length();

    // Per the style specification, the "global-state" property name must be
    // a string literal, not the result of a sub-expression.
    if overloads.iter().any(|overload| overload.role == Role::GlobalState)
        && length == 2
        && value.array_member(1).to_string().is_none()
    {
        ctx.error_at("Global state property must be a string literal.".to_owned(), 1);
        return None;
    }

    // Each failed overload parses its arguments, so two overloads of one arity
    // would make parsing a nested chain of them exponential in its depth.
    let mut overload_matched_arity = false;
    for overload in overloads {
        let matches = match &overload.params {
            Params::Varargs(_) => true,
            Params::Fixed(p) => p.len() == length - 1,
        };
        if matches {
            debug_assert!(!overload_matched_arity);
            overload_matched_arity = true;

            // First parse all the args, potentially coercing to the types
            // expected by this overload.
            ctx.clear_errors();
            let mut arg_parse_failed = false;
            let mut args = Vec::with_capacity(length - 1);

            for i in 1..length {
                let expected = match &overload.params {
                    Params::Varargs(t) => t.clone(),
                    Params::Fixed(params) => params[i - 1].clone(),
                };

                let Some(parsed) = ctx.parse(&value.array_member(i), i, Some(expected), None) else {
                    arg_parse_failed = true;
                    break;
                };
                args.push(parsed);
            }
            // When the args couldn't be coerced to this overload's expected
            // types, move on to the next one.
            if !arg_parse_failed {
                let parse_with_args = create_compound_expression_from(overloads, args, ctx);
                if parse_with_args.is_some() {
                    return parse_with_args;
                }
            }
        }
    }
    // The overload that takes this many arguments failed, and its errors say
    // why. Parsing the arguments again would double the work at every level.
    if overload_matched_arity {
        return None;
    }

    // No overload takes this many arguments. Parse the arguments without
    // expected types just for the error message
    ctx.clear_errors();
    let mut args = Vec::with_capacity(length - 1);

    for i in 1..length {
        let parsed = ctx.parse(&value.array_member(i), i, None, None)?;
        args.push(parsed);
    }

    ctx.error(expected_types_error(overloads, &args));

    None
}

/// The call of the function `name` of `ctx`'s functions with `args`, through
/// the first of its overloads whose parameters the arguments' types match.
pub fn create_compound_expression(name: &str, args: Vec<Expr>, ctx: &mut ParsingContext) -> ParseResult {
    let functions = Arc::clone(ctx.functions());
    let overloads = functions.get(name).map_or(&[][..], FunctionDef::overloads);
    create_compound_expression_from(overloads, args, ctx)
}
