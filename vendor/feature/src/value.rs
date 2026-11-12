// Ports mapbox::base::Value / mapbox::feature::value (what mln::Value in
// include/mln/util/feature.hpp aliases): a JSON-like variant that, unlike
// serde_json::Value, distinguishes uint64/int64/double — the expression and
// conversion subsystems depend on that distinction.

use serde::{Deserialize, Serialize};

// C++ uses std::unordered_map; a VecMap gives us deterministic iteration in
// key order (useful for serialization comparisons) at no behavioral cost.
pub type ValueObject = mln_vec_map::VecMap<String, Value>;

/// `mapbox::base::Value`:
/// `variant<null, bool, uint64_t, int64_t, double, string, vector, map>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Uint(u64),
    Int(i64),
    Double(f64),
    String(String),
    Array(Vec<Value>),
    Object(ValueObject),
}

impl Value {
    /// Numeric accessor mirroring `Value::getDouble()`-style coercion used by
    /// conversions: any of the three numeric alternatives as f64.
    #[must_use]
    pub fn as_double(&self) -> Option<f64> {
        match self {
            Value::Uint(v) => Some(*v as f64),
            Value::Int(v) => Some(*v as f64),
            Value::Double(v) => Some(*v),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::Uint(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Double(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_owned())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}
impl From<ValueObject> for Value {
    fn from(v: ValueObject) -> Self {
        Value::Object(v)
    }
}
