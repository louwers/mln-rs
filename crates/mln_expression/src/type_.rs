// The module is `type_` because `type` is a Rust keyword. The C++ constants
// (`type::Number`, ...) are the enum variants (`Type::Number`, ...).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The type of an expression's value, with the variants in the C++ variant's
/// order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    Null,
    Number,
    Boolean,
    String,
    Color,
    Padding,
    ProjectionDefinition,
    VariableAnchorOffsetCollection,
    Object,
    Value,
    Array(Box<Array>),
    Collator,
    Formatted,
    Error,
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Array {
    pub item_type: Type,
    pub n: Option<usize>,
}

impl Array {
    #[must_use]
    pub(crate) fn new(item_type: Type) -> Self {
        Array { item_type, n: None }
    }

    #[must_use]
    pub(crate) fn with_n(item_type: Type, n: Option<usize>) -> Self {
        Array { item_type, n }
    }

    #[must_use]
    pub(crate) fn get_name(&self) -> String {
        if let Some(n) = self.n {
            format!("array<{}, {}>", to_string(&self.item_type), n)
        } else if self.item_type == Type::Value {
            "array".to_owned()
        } else {
            format!("array<{}>", to_string(&self.item_type))
        }
    }
}

impl From<Array> for Type {
    fn from(a: Array) -> Self {
        Type::Array(Box::new(a))
    }
}

impl Type {
    #[must_use]
    pub fn array(item_type: Type) -> Type {
        Array::new(item_type).into()
    }

    #[must_use]
    pub fn array_n(item_type: Type, n: usize) -> Type {
        Array::with_n(item_type, Some(n)).into()
    }

    #[must_use]
    pub(crate) fn is_array(&self) -> bool {
        matches!(self, Type::Array(_))
    }

    #[must_use]
    pub(crate) fn as_array(&self) -> Option<&Array> {
        match self {
            Type::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) fn get_name(&self) -> String {
        match self {
            Type::Null => "null".to_owned(),
            Type::Number => "number".to_owned(),
            Type::Boolean => "boolean".to_owned(),
            Type::String => "string".to_owned(),
            Type::Color => "color".to_owned(),
            Type::Padding => "padding".to_owned(),
            Type::ProjectionDefinition => "projectionDefinition".to_owned(),
            Type::VariableAnchorOffsetCollection => "variableAnchorOffsetCollection".to_owned(),
            Type::Object => "object".to_owned(),
            Type::Value => "value".to_owned(),
            Type::Array(a) => a.get_name(),
            Type::Collator => "collator".to_owned(),
            Type::Formatted => "formatted".to_owned(),
            Type::Error => "error".to_owned(),
            Type::Image => "resolvedImage".to_owned(),
        }
    }
}

#[must_use]
pub fn to_string(t: &Type) -> String {
    t.get_name()
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.get_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(to_string(&Type::array(Type::Value)), "array");
        assert_eq!(to_string(&Type::array(Type::Number)), "array<number>");
        assert_eq!(to_string(&Type::array_n(Type::Value, 2)), "array<value, 2>");
        assert_eq!(to_string(&Type::Image), "resolvedImage");
    }
}
