// Ports include/mln/style/conversion/get_json_type.hpp and
// src/mln/style/conversion/get_json_type.cpp.

use feature::Value;

use crate::conversion::Convertible;

#[must_use]
pub fn get_json_type(value: &Convertible<'_>) -> String {
    if value.is_undefined() {
        return "null".to_owned();
    }
    if value.is_array() {
        return "array".to_owned();
    }
    if value.is_object() {
        return "object".to_owned();
    }
    let v = value.to_value();

    // Since we've already checked the non-atomic types above, value must then
    // be a string, number, or boolean -- thus, assume that the toValue()
    // conversion succeeds.
    match v.expect("to_value of a non-null scalar") {
        Value::String(_) => "string",
        Value::Bool(_) => "boolean",
        _ => "number",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn types() {
        for (v, t) in [
            (json!(null), "null"),
            (json!([]), "array"),
            (json!({}), "object"),
            (json!("x"), "string"),
            (json!(false), "boolean"),
            (json!(1), "number"),
            (json!(-1.5), "number"),
        ] {
            assert_eq!(get_json_type(&Convertible::from(&v)), t);
        }
    }
}
