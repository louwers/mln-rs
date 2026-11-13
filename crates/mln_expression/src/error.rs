use feature::Value as MbglValue;

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind,
};
use crate::type_::Type;
use crate::value::Value;

pub(crate) struct Error {
    base: ExpressionBase,
    message: String,
}

impl Error {
    #[must_use]
    pub(crate) fn new(message: String) -> Self {
        Error { base: ExpressionBase::new(Kind::Error, Type::Error, Dependency::NONE), message }
    }
}

impl Expression for Error {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, _: &EvaluationContext<'_>) -> EvaluationResult {
        Err(EvaluationError::new(self.message.clone()))
    }

    fn each_child<'a>(&'a self, _: &mut dyn FnMut(&'a dyn Expression)) {}

    fn equals(&self, e: &dyn Expression) -> bool {
        e.get_kind() == Kind::Error
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        Vec::new()
    }

    /// Port-only: C++ serializes this as `["error"]` (the default), which
    /// doesn't parse back, since the "error" expression takes its message.
    /// The legacy property functions put an `Error` where a categorical or
    /// interval function has no default, so their layers' JSON would not
    /// parse back.
    fn serialize(&self) -> MbglValue {
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), MbglValue::String(self.message.clone())])
    }

    fn get_operator(&self) -> String {
        "error".to_owned()
    }
}
