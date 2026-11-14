use std::borrow::Cow;

use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::check_subtype::check_subtype;
use crate::compound_expression::Op;
use crate::expression::{EvalResult, EvaluationContext, EvaluationError, EvaluationResult, Expr, Node, Output};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, type_of};

/// `["number", ...]` and the other type assertions: the first input whose
/// value has the type, or an error.
#[derive(Clone, PartialEq)]
pub(crate) struct Assertion {
    type_: Type,
    inputs: Box<[Expr]>,
    /// Whether each input has the asserted type, so its typed evaluation
    /// needs no check.
    typed: Box<[bool]>,
}

fn assertion_type(name: &str) -> Option<Type> {
    match name {
        // TODO: give "image" its own type once we implement image expressions
        "string" | "image" => Some(Type::String),
        "number" => Some(Type::Number),
        "boolean" => Some(Type::Boolean),
        "object" => Some(Type::Object),
        _ => None,
    }
}

impl Assertion {
    pub(crate) fn new_expr(type_: Type, inputs: Vec<Expr>) -> Expr {
        assert!(!inputs.is_empty(), "Assertion without inputs");
        let typed = inputs.iter().map(|input| input.get_type() == type_).collect();
        Expr::new(Node::Assertion(Box::new(Assertion { type_, inputs: inputs.into(), typed })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    pub(crate) fn inputs(&self) -> &[Expr] {
        &self.inputs
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let length = value.array_length();

        if length < 2 {
            ctx.error("Expected at least one argument.".to_owned());
            return None;
        }

        let mut i = 1;

        let name = value.array_member(0).to_string().expect("operator name");
        let type_ = if name == "array" {
            let item_type;
            if length > 2 {
                let item_type_name = value.array_member(1).to_string();
                let it = item_type_name.as_deref().and_then(assertion_type);
                match it {
                    Some(t) if t != Type::Object => item_type = t,
                    _ => {
                        ctx.error_at(
                            r#"The item type argument of "array" must be one of string, number, boolean"#.to_owned(),
                            1,
                        );
                        return None;
                    }
                }
                i += 1;
            } else {
                item_type = Type::Value;
            }

            let mut n = None;
            if length > 3 {
                let m = value.array_member(2);
                let num = m.to_number();
                if !m.is_undefined() && num.is_none_or(|v| v < 0.0 || v != v.floor()) {
                    ctx.error_at(r#"The length argument to "array" must be a positive integer literal."#.to_owned(), 2);
                    return None;
                }
                if let Some(num) = num {
                    n = Some(num as usize);
                }
                i += 1;
            }

            type_::Array::with_n(item_type, n).into()
        } else {
            assertion_type(&name).expect("assertion type")
        };

        let mut parsed = Vec::with_capacity(length - 1);
        while i < length {
            let input = ctx.parse(&value.array_member(i), i, Some(Type::Value), None)?;
            parsed.push(input);
            i += 1;
        }

        Some(Assertion::new_expr(type_, parsed))
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        // The parser makes an assertion of at least one input.
        let mut mismatch = Value::Null;
        for input in &self.inputs {
            let value = input.evaluate(params)?;
            if check_subtype(&self.type_, &type_of(&value)).is_none() {
                return Ok(value);
            }
            mismatch = value;
        }
        Err(self.mismatch(&mismatch))
    }

    fn mismatch(&self, value: &Value) -> EvaluationError {
        EvaluationError::new(format!(
            "Expected value to be of type {}, but found {} instead.",
            type_::to_string(&self.type_),
            type_::to_string(&type_of(value))
        ))
    }

    /// The typed evaluations: an input of the asserted type evaluates typed,
    /// and a feature accessor (`["get", k]`, `["id"]`) is read without
    /// building a value first.
    fn eval_typed<'e, T: Output<'e>>(
        &'e self,
        params: &EvaluationContext<'_>,
        from_feature: impl Fn(&MbglValue) -> Option<T>,
    ) -> EvalResult<T> {
        let mut mismatch = Value::Null;
        for (input, &typed) in self.inputs.iter().zip(&self.typed) {
            if typed {
                return T::eval(input, params);
            }
            if let Some(read) = read_feature(input, params) {
                match read? {
                    Some(value) => match from_feature(&value) {
                        Some(typed) => return Ok(typed),
                        None => mismatch = crate::value::from_mbgl_value_owned(value),
                    },
                    None => mismatch = Value::Null,
                }
                continue;
            }
            let value = input.evaluate(params)?;
            if check_subtype(&self.type_, &type_of(&value)).is_none() {
                return T::from_value(value);
            }
            mismatch = value;
        }
        Err(self.mismatch(&mismatch))
    }

    pub(crate) fn eval_num(&self, params: &EvaluationContext<'_>) -> EvalResult<f64> {
        self.eval_typed(params, |v| match *v {
            MbglValue::Double(n) => Some(n),
            MbglValue::Uint(n) => Some(n as f64),
            MbglValue::Int(n) => Some(n as f64),
            _ => None,
        })
    }

    pub(crate) fn eval_bool(&self, params: &EvaluationContext<'_>) -> EvalResult<bool> {
        self.eval_typed(params, |v| match *v {
            MbglValue::Bool(b) => Some(b),
            _ => None,
        })
    }

    pub(crate) fn eval_str<'e>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
        self.eval_typed(params, |v| match v {
            MbglValue::String(s) => Some(Cow::Owned(s.clone())),
            _ => None,
        })
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator().to_owned())];

        if let Type::Array(array) = &self.type_
            && matches!(array.item_type, Type::String | Type::Number | Type::Boolean)
        {
            serialized.push(MbglValue::String(type_::to_string(&array.item_type)));
            if let Some(n) = array.n {
                serialized.push(MbglValue::Uint(n as u64));
            } else if self.inputs.len() > 1 {
                serialized.push(MbglValue::Null);
            }
        }

        for input in &self.inputs {
            serialized.push(input.serialize());
        }

        MbglValue::Array(serialized)
    }

    pub(crate) fn get_operator(&self) -> &'static str {
        match self.type_ {
            Type::Array(_) => "array",
            Type::String => "string",
            Type::Number => "number",
            Type::Boolean => "boolean",
            Type::Object => "object",
            _ => unreachable!("assertion to {}", self.type_),
        }
    }
}

/// The feature value or id that `input` reads, when it is `["get", key]`
/// with a literal key or `["id"]`: `Some(Ok(None))` when the feature lacks
/// it, and `None` when `input` is another expression.
fn read_feature(input: &Expr, params: &EvaluationContext<'_>) -> Option<EvalResult<Option<MbglValue>>> {
    let compound = input.as_compound()?;
    match compound.signature().op {
        Op::Get => {
            let key = compound.args()[0].literal_value()?.as_str()?;
            Some(match params.feature {
                Some(feature) => Ok(feature.get_value(key)),
                None => Err(crate::compound_expression::feature_unavailable()),
            })
        }
        Op::Id => Some(match params.feature {
            Some(feature) => Ok(crate::compound_expression::identifier_to_mbgl(feature.get_id())),
            None => Err(crate::compound_expression::feature_unavailable()),
        }),
        _ => None,
    }
}
