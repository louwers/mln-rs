use mln_conversion::Convertible;

use crate::expression::{EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_;
use crate::type_::Type;
use crate::utf8_op_helpers::{get_unicode_character_offset_on_validated_utf8, unicode_length_on_validated_utf8};
use crate::value::{Value, type_of};

fn normalize_index(index: i32, length: i32) -> i32 {
    if index < 0 { index.wrapping_add(length) } else { index }
}

/// Truncates `f` to `i32` as C++ `static_cast<int>(double)` does when clang
/// compiles it on x86-64: out of range and NaN give `i32::MIN`.
fn static_cast_int(f: f64) -> i32 {
    if f > -2_147_483_649.0 && f < 2_147_483_648.0 { f as i32 } else { i32::MIN }
}

/// `["slice", input, from, to?]`, of a string or an array.
#[derive(Clone, PartialEq)]
pub(crate) struct Slice {
    input: Expr,
    from_index: Expr,
    to_index: Option<Expr>,
}

impl Slice {
    /// The input's type.
    pub(crate) fn get_type(&self) -> Type {
        self.input.get_type()
    }

    fn evaluate_for_array_input(array: &[Value], from_index_value: i32, to_index_value: i32) -> Value {
        let length = array.len() as i32;
        let mut to_index_value = to_index_value;
        if to_index_value == i32::MAX {
            to_index_value = length;
        }
        let from_index_value = normalize_index(from_index_value, length);
        to_index_value = normalize_index(to_index_value, length);

        if from_index_value >= to_index_value {
            return Value::Array(Vec::new());
        }
        to_index_value = to_index_value.min(length);

        // C++ reads `array[index]` for a negative normalized
        // `fromIndex` (undefined behaviour); the port starts at 0.
        let mut result = Vec::with_capacity((to_index_value - from_index_value) as usize);
        for index in from_index_value.max(0)..to_index_value {
            result.push(array[index as usize].clone());
        }
        Value::Array(result)
    }

    fn evaluate_for_string_input(string: &str, from_index_value: i32, to_index_value: i32) -> Value {
        let length_bytes = string.len();
        let length = unicode_length_on_validated_utf8(string) as i32;
        let to_index_value_bytes = if to_index_value == i32::MAX {
            length_bytes
        } else {
            let to_index_value_chars = normalize_index(to_index_value, length);
            // A negative int converts to a huge size_t.
            get_unicode_character_offset_on_validated_utf8(string, to_index_value_chars as isize as usize)
        };
        let from_index_value_chars = normalize_index(from_index_value, length);
        let from_index_value_bytes =
            get_unicode_character_offset_on_validated_utf8(string, from_index_value_chars as isize as usize);

        if from_index_value_bytes >= length_bytes || from_index_value_bytes >= to_index_value_bytes {
            return Value::String(String::new());
        }

        Value::String(string[from_index_value_bytes..to_index_value_bytes].to_owned())
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();
        if length != 3 && length != 4 {
            ctx.error(format!("Expected 2 or 3 arguments, but found {} instead.", length - 1));
            return None;
        }

        let input = ctx.parse(&value.array_member(1), 1, None, None);
        let from_index = ctx.parse(&value.array_member(2), 2, None, None);

        let to_index = if length == 4 { ctx.parse(&value.array_member(3), 3, None, None) } else { None };

        let (Some(input), Some(from_index)) = (input, from_index) else {
            return None;
        };

        Some(Expr::new(Node::Slice(Box::new(Slice { input, from_index, to_index }))))
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_input = self.input.evaluate_ref(params)?;
        // C++ checks `!fromIndex` (the expression, never null)
        // instead of the result here and then dereferences a failed result
        // (undefined behaviour); the port returns the error.
        let evaluated_from_index = self.from_index.evaluate(params)?;
        let Value::Number(from) = evaluated_from_index else {
            return Err(EvaluationError::new(format!(
                "Expected value to be of type number, but found {} instead.",
                type_::to_string(&type_of(&evaluated_from_index))
            )));
        };

        let from_index_value = static_cast_int(from);
        let mut to_index_value = i32::MAX;

        if let Some(to_index) = &self.to_index {
            let evaluated_to_index = to_index.evaluate(params)?;
            let Value::Number(to) = evaluated_to_index else {
                // Quirk kept from the C++: reports the type of fromIndex.
                return Err(EvaluationError::new(format!(
                    "Expected value to be of type number, but found {} instead.",
                    type_::to_string(&type_of(&evaluated_from_index))
                )));
            };
            to_index_value = static_cast_int(to);
        }

        match &*evaluated_input {
            Value::String(s) => Ok(Self::evaluate_for_string_input(s, from_index_value, to_index_value)),
            Value::Array(v) => Ok(Self::evaluate_for_array_input(v, from_index_value, to_index_value)),
            _ => Err(EvaluationError::new(format!(
                "Expected first argument to be of type array or string, but found {} instead.",
                type_::to_string(&type_of(&evaluated_input))
            ))),
        }
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.input);
        visit(&self.from_index);
        if let Some(to_index) = &self.to_index {
            visit(to_index);
        }
    }
}
