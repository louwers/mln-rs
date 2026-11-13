use mln_conversion::Convertible;

use crate::compound_expression::CompoundExpression;
use crate::parsing_context::is_expression_name;

#[must_use]
pub fn is_expression(value: &Convertible<'_>) -> bool {
    if !value.is_array() || value.array_length() == 0 {
        return false;
    }
    let Some(name) = value.array_member(0).to_string() else {
        return false;
    };

    is_expression_name(&name) || CompoundExpression::exists(&name)
}
