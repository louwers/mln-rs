// Differences that come from the JSON library, not from this file:
// serde_json parses the integer literal `-0` as the double
// -0.0 (rapidjson: unsigned 0), rounds long decimals correctly (rapidjson's
// default parse can be off by one ULP), and, without `preserve_order`,
// iterates object members in key order (rapidjson: document order).
//
// The array and object accessors have preconditions that rapidjson only
// asserts (undefined behaviour with NDEBUG): a non-array has length 0, a
// member past the end or of a non-array is null (undefined), and a
// non-object has no members. So a converter that skips a check (the C++
// `isExpression` reads `["in"]`'s second member) gets an error or a
// default, not a panic.

use feature::Value;

use crate::conversion::{ConversionTraits, Convertible, Error};

static NULL: serde_json::Value = serde_json::Value::Null;

impl ConversionTraits for serde_json::Value {
    fn is_undefined(&self) -> bool {
        self.is_null()
    }

    fn is_array(&self) -> bool {
        self.is_array()
    }

    fn array_length(&self) -> usize {
        match self {
            serde_json::Value::Array(a) => a.len(),
            _ => 0,
        }
    }

    fn array_member(&self, i: usize) -> Convertible<'_> {
        match self {
            serde_json::Value::Array(a) => Convertible::new(a.get(i).unwrap_or(&NULL)),
            _ => Convertible::new(&NULL),
        }
    }

    fn is_object(&self) -> bool {
        self.is_object()
    }

    fn object_member(&self, name: &str) -> Option<Convertible<'_>> {
        match self {
            serde_json::Value::Object(o) => o.get(name).map(|m| Convertible::new(m)),
            _ => None,
        }
    }

    fn each_member(&self, f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
        let serde_json::Value::Object(o) = self else {
            return None;
        };
        for (name, value) in o {
            let result = f(name, &Convertible::new(value));
            if result.is_some() {
                return result;
            }
        }
        None
    }

    fn to_bool(&self) -> Option<bool> {
        self.as_bool()
    }

    fn to_number(&self) -> Option<f32> {
        // static_cast<float>(value->GetDouble())
        self.as_f64().map(|d| d as f32)
    }

    fn to_double(&self) -> Option<f64> {
        self.as_f64()
    }

    fn to_string(&self) -> Option<String> {
        self.as_str().map(str::to_owned)
    }

    fn to_value(&self) -> Option<Value> {
        match self {
            // Quirk kept from the C++: null converts to `false`.
            serde_json::Value::Null | serde_json::Value::Bool(false) => Some(Value::Bool(false)),
            serde_json::Value::Bool(true) => Some(Value::Bool(true)),
            serde_json::Value::String(s) => Some(Value::String(s.clone())),
            serde_json::Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    Some(Value::Uint(u))
                } else if let Some(i) = n.as_i64() {
                    Some(Value::Int(i))
                } else {
                    n.as_f64().map(Value::Double)
                }
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
        }
    }

    fn to_geojson(&self, error: &mut Error) -> Option<geojson::GeoJSON> {
        to_geojson(self, error)
    }
}

/// Converts `value` to GeoJSON, storing the conversion's error message in
/// `error` when it fails.
pub(crate) fn to_geojson(value: &serde_json::Value, error: &mut Error) -> Option<geojson::GeoJSON> {
    match geojson::convert(value) {
        Ok(geojson) => Some(geojson),
        Err(e) => {
            error.message = e.message().to_owned();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn primitives() {
        let v = json!({"a": [1, -2, 3.5, "s", true, null], "b": {"c": 1}});
        let c = Convertible::from(&v);
        assert!(c.is_object());
        assert!(!c.is_array());
        assert!(c.object_member("missing").is_none());
        let a = c.object_member("a").unwrap();
        assert!(a.is_array());
        assert_eq!(a.array_length(), 6);
        assert_eq!(a.array_member(0).to_value(), Some(Value::Uint(1)));
        assert_eq!(a.array_member(1).to_value(), Some(Value::Int(-2)));
        assert_eq!(a.array_member(2).to_value(), Some(Value::Double(3.5)));
        assert_eq!(a.array_member(2).to_number(), Some(3.5f32));
        assert_eq!(a.array_member(3).to_string(), Some("s".to_owned()));
        assert_eq!(a.array_member(3).to_number(), None);
        assert_eq!(a.array_member(4).to_bool(), Some(true));
        assert!(a.array_member(5).is_undefined());
        assert_eq!(a.array_member(5).to_value(), Some(Value::Bool(false)));
        assert_eq!(a.to_value(), None);
    }

    #[test]
    fn each_member_short_circuits() {
        let v = json!({"a": 1, "b": 2, "c": 3});
        let c = Convertible::from(&v);
        let mut seen = 0;
        let result = c.each_member(|name, _| {
            seen += 1;
            (name == "b").then(|| Error::new("stop"))
        });
        assert_eq!(result, Some(Error::new("stop")));
        assert_eq!(seen, 2);
    }

    // Port-only: rapidjson asserts these preconditions; here they are total.
    #[test]
    fn accessors_outside_their_preconditions() {
        let number = serde_json::json!(1);
        let c = Convertible::from(&number);
        assert_eq!(c.array_length(), 0);
        assert!(c.array_member(0).is_undefined());
        assert!(c.object_member("a").is_none());
        assert_eq!(c.each_member(|_, _| Some(Error::new("member"))), None);
        let array = serde_json::json!([1]);
        assert!(Convertible::from(&array).array_member(1).is_undefined());
    }

    #[test]
    fn owned() {
        struct Wrapped(serde_json::Value);
        impl ConversionTraits for Wrapped {
            fn is_undefined(&self) -> bool {
                self.0.is_undefined()
            }
            fn is_array(&self) -> bool {
                ConversionTraits::is_array(&self.0)
            }
            fn array_length(&self) -> usize {
                self.0.array_length()
            }
            fn array_member(&self, i: usize) -> Convertible<'_> {
                self.0.array_member(i)
            }
            fn is_object(&self) -> bool {
                ConversionTraits::is_object(&self.0)
            }
            fn object_member(&self, name: &str) -> Option<Convertible<'_>> {
                self.0.object_member(name)
            }
            fn each_member(&self, f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
                self.0.each_member(f)
            }
            fn to_bool(&self) -> Option<bool> {
                self.0.to_bool()
            }
            fn to_number(&self) -> Option<f32> {
                self.0.to_number()
            }
            fn to_double(&self) -> Option<f64> {
                self.0.to_double()
            }
            fn to_string(&self) -> Option<String> {
                ConversionTraits::to_string(&self.0)
            }
            fn to_value(&self) -> Option<Value> {
                self.0.to_value()
            }
        }
        let c = Convertible::owned(Wrapped(json!([7])));
        assert_eq!(c.array_member(0).to_double(), Some(7.0));
    }
}
