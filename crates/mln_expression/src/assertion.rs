use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::check_subtype::check_subtype;
use crate::expression::{
    EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, children_equal,
    collect_dependencies, downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, type_of};

pub(crate) struct Assertion {
    base: ExpressionBase,
    inputs: Vec<Box<dyn Expression>>,
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
    #[must_use]
    pub(crate) fn new(type_: Type, inputs: Vec<Box<dyn Expression>>) -> Self {
        assert_ne!(inputs, Vec::<Box<dyn Expression>>::new());
        Assertion { base: ExpressionBase::new(Kind::Assertion, type_, collect_dependencies(&inputs)), inputs }
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

        Some(Box::new(Assertion::new(type_, parsed)))
    }
}

impl Expression for Assertion {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        for (i, input) in self.inputs.iter().enumerate() {
            let value = input.evaluate(params)?;
            if check_subtype(self.get_type(), &type_of(&value)).is_none() {
                return Ok(value);
            } else if i == self.inputs.len() - 1 {
                return Err(EvaluationError::new(format!(
                    "Expected value to be of type {}, but found {} instead.",
                    type_::to_string(self.get_type()),
                    type_::to_string(&type_of(&value))
                )));
            }
        }

        unreachable!("Assertion without inputs")
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for input in &self.inputs {
            visit(&**input);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Assertion {
            let rhs = downcast::<Assertion>(e).expect("Assertion");
            return self.get_type() == rhs.get_type() && children_equal(&self.inputs, &rhs.inputs);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for input in &self.inputs {
            result.extend(input.possible_outputs());
        }
        result
    }

    fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator())];

        if let Type::Array(array) = self.get_type()
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

    fn get_operator(&self) -> String {
        if self.get_type().is_array() { "array".to_owned() } else { type_::to_string(self.get_type()) }
    }
}
