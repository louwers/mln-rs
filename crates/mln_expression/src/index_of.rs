use mln_conversion::Convertible;
use mln_util::string::to_string_double;

use crate::expression::{EvalResult, EvaluationContext, EvaluationError, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_;
use crate::utf8_op_helpers::{get_unicode_character_offset_on_validated_utf8, unicode_length_on_validated_utf8};
use crate::value::{Value, type_of};

/// `["index-of", keyword, input, from_index?]`.
#[derive(Clone, PartialEq)]
pub(crate) struct IndexOf {
    keyword: Expr,
    input: Expr,
    from_index: Option<Expr>,
}

impl IndexOf {
    fn evaluate_for_array_input(array: &[Value], keyword_value: &Value, from_index_value: usize) -> f64 {
        for (index, value) in array.iter().enumerate().skip(from_index_value) {
            if *value == *keyword_value {
                return index as f64;
            }
        }
        -1.0
    }

    fn evaluate_for_string_input(string: &str, keyword_value: &Value, from_index_value: usize) -> f64 {
        let keyword_string = match keyword_value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_owned(),
            Value::Null => "null".to_owned(),
            Value::Number(n) => to_string_double(*n),
            _ => {
                // it should be impossible to get here - we do validation in
                // evaluate()
                debug_assert!(false);
                String::new()
            }
        };
        let from_index_byte_offset = get_unicode_character_offset_on_validated_utf8(string, from_index_value);
        let Some(byte_index) =
            string[from_index_byte_offset..].find(keyword_string.as_str()).map(|i| i + from_index_byte_offset)
        else {
            return -1.0;
        };
        let prefix = &string[..byte_index];
        let index = unicode_length_on_validated_utf8(prefix);
        index as f64
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();
        if length != 3 && length != 4 {
            ctx.error(format!("Expected 2 or 3 arguments, but found {} instead.", length - 1));
            return None;
        }

        let keyword = ctx.parse(&value.array_member(1), 1, None, None);
        let input = ctx.parse(&value.array_member(2), 2, None, None);

        let from_index = if length == 4 { ctx.parse(&value.array_member(3), 3, None, None) } else { None };

        let (Some(keyword), Some(input)) = (keyword, input) else {
            return None;
        };

        Some(Expr::new(Node::IndexOf(Box::new(IndexOf { keyword, input, from_index }))))
    }

    pub(crate) fn eval_num(&self, params: &EvaluationContext<'_>) -> EvalResult<f64> {
        let evaluated_keyword = self.keyword.evaluate_ref(params);
        let evaluated_input = self.input.evaluate_ref(params);
        let evaluated_keyword = evaluated_keyword?;
        let evaluated_input = evaluated_input?;

        if !matches!(*evaluated_keyword, Value::Number(_) | Value::String(_) | Value::Bool(_) | Value::Null) {
            return Err(EvaluationError::new(format!(
                "Expected first argument to be of type boolean, string, number or null, but found {} instead.",
                type_::to_string(&type_of(&evaluated_keyword))
            )));
        }

        let mut from_index_value: usize = 0;
        if let Some(from_index) = &self.from_index {
            let evaluated_from_index = from_index.evaluate(params)?;
            let Value::Number(f) = evaluated_from_index else {
                return Err(EvaluationError::new(format!(
                    "Expected third argument to be of type number, but found {} instead.",
                    type_::to_string(&type_of(&evaluated_from_index))
                )));
            };
            // static_cast<size_t>(std::max(f, 0.0))
            let f = if f < 0.0 { 0.0 } else { f };
            from_index_value = f as usize;
        }

        match &*evaluated_input {
            Value::String(s) => Ok(Self::evaluate_for_string_input(s, &evaluated_keyword, from_index_value)),
            Value::Array(v) => Ok(Self::evaluate_for_array_input(v, &evaluated_keyword, from_index_value)),
            _ => Err(EvaluationError::new(format!(
                "Expected second argument to be of type array or string, but found {} instead.",
                type_::to_string(&type_of(&evaluated_input))
            ))),
        }
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.keyword);
        visit(&self.input);
        if let Some(from_index) = &self.from_index {
            visit(from_index);
        }
    }
}
