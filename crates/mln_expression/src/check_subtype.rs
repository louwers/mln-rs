// Ports include/mln/style/expression/check_subtype.hpp and
// src/mln/style/expression/check_subtype.cpp.

use crate::type_::{self, Array, Type};

#[must_use]
pub fn error_message(expected: &Type, t: &Type) -> String {
    format!("Expected {} but found {} instead.", type_::to_string(expected), type_::to_string(t))
}

/// `type::checkSubtype`: `None` if `t` is a subtype of `expected`, else the
/// error message.
// `if err.is_none() { return None; }` is C++'s `if (!err) return {};`: a
// member matched, so `None` here is success, not an error to pass on.
#[allow(clippy::question_mark)]
#[must_use]
pub fn check_subtype(expected: &Type, t: &Type) -> Option<String> {
    if *t == Type::Error {
        return None;
    }

    match expected {
        Type::Array(expected_array) => {
            let Type::Array(actual_array) = t else {
                return Some(error_message(expected, t));
            };
            if actual_array.n.is_none_or(|n| n != 0) || actual_array.item_type != Type::Value {
                let err = check_subtype(&expected_array.item_type, &actual_array.item_type);
                if err.is_some() {
                    return Some(error_message(expected, t));
                }
            }
            if expected_array.n.is_some() && expected_array.n != actual_array.n {
                return Some(error_message(expected, t));
            }
            None
        }
        Type::Value => {
            if *t == Type::Value {
                return None;
            }

            let members = [
                Type::Null,
                Type::Boolean,
                Type::Number,
                Type::String,
                Type::Object,
                Type::Color,
                Type::Padding,
                Type::ProjectionDefinition,
                Type::Formatted,
                Type::Image,
                Array::new(Type::Value).into(),
            ];

            for member in &members {
                let err = check_subtype(member, t);
                if err.is_none() {
                    return None;
                }
            }
            Some(error_message(expected, t))
        }
        _ => {
            if expected != t {
                return Some(error_message(expected, t));
            }
            None
        }
    }
}
