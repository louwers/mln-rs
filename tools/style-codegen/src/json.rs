// `JSON.stringify` for the spec values the templates embed (the
// heatmap-color default in layer.cpp.ejs).

use serde_json::Value;

use crate::defaults::js_number;

/// `JSON.stringify(value)`: compact, object members in document order, JS
/// number formatting.
pub(crate) fn js_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => js_number(i as f64),
            (_, Some(u)) => js_number(u as f64),
            _ => js_number(n.as_f64().expect("number")),
        },
        // serde_json escapes like JSON.stringify for the characters the spec
        // uses (quote, backslash, control characters).
        Value::String(s) => serde_json::to_string(s).expect("string"),
        Value::Array(items) => format!("[{}]", items.iter().map(js_stringify).collect::<Vec<_>>().join(",")),
        Value::Object(members) => format!(
            "{{{}}}",
            members
                .iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).expect("key"), js_stringify(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}
