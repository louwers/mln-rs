use mln_conversion::Convertible;

use crate::functions::Functions;
use crate::parsing_context::is_special_form;

/// Whether `value` is an expression: an array whose first member names an
/// expression form or one of `functions`.
#[must_use]
pub fn is_expression(value: &Convertible<'_>, functions: &Functions) -> bool {
    if !value.is_array() || value.array_length() == 0 {
        return false;
    }
    let Some(name) = value.array_member(0).to_string() else {
        return false;
    };

    is_special_form(&name) || functions.contains(&name)
}
