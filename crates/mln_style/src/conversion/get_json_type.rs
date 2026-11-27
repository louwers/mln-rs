use super::Convertible;
use crate::feature::Value;

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
    match value.to_value() {
        None | Some(Value::Null) => "null",
        Some(Value::String(_)) => "string",
        Some(Value::Bool(_)) => "boolean",
        Some(_) => "number",
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
