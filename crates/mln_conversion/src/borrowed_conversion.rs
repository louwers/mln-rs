// The front end over `serde_json_borrow`'s tree, which the style parser
// reads a style's text into. A string that needs no unescaping borrows from
// the text, and an object is a Vec of its members in document order, so
// building the tree allocates little and hashes nothing.
//
// An object can name a key twice. It reads as `JSON.parse` and serde_json's
// map read it: each key once, at its first member's position, with its last
// member's value.

use std::collections::HashSet;

use feature::Value;
use geojson::GeoJson;
pub use serde_json_borrow::{Map as JsonObject, Value as Json};

use crate::conversion::{ConversionTraits, Convertible, Error};

/// What a member past the end of an array reads as, as in the other front
/// ends.
static NULL: Json<'static> = Json::Null;

/// The value of `object`'s member `key`; of members with that key, the last.
#[must_use]
pub fn member<'a, 'ctx>(object: &'a JsonObject<'ctx>, key: &str) -> Option<&'a Json<'ctx>> {
    object.as_vec().iter().rev().find(|(name, _)| **name == *key).map(|(_, value)| value)
}

/// The members of `object` in document order, each key once: at its first
/// member's position, with its last member's value.
pub fn members<'a, 'ctx>(object: &'a JsonObject<'ctx>) -> impl Iterator<Item = (&'a str, &'a Json<'ctx>)> {
    let entries = object.as_vec();
    let (unique, deduplicated) = if has_duplicate_keys(object) {
        let mut seen = HashSet::new();
        let deduplicated: Vec<_> = entries
            .iter()
            .filter(|(name, _)| seen.insert(&**name))
            .filter_map(|(name, _)| Some((&**name, member(object, name)?)))
            .collect();
        (None, Some(deduplicated))
    } else {
        (Some(entries.iter().map(|(name, value)| (&**name, value))), None)
    };
    unique.into_iter().flatten().chain(deduplicated.into_iter().flatten())
}

fn has_duplicate_keys(object: &JsonObject<'_>) -> bool {
    let entries = object.as_vec();
    // Comparing each pair is faster than hashing for the small objects of a
    // style.
    if entries.len() <= 16 {
        entries.iter().enumerate().any(|(i, (name, _))| entries[..i].iter().any(|(earlier, _)| earlier == name))
    } else {
        let mut seen = HashSet::with_capacity(entries.len());
        !entries.iter().all(|(name, _)| seen.insert(&**name))
    }
}

impl ConversionTraits for Json<'_> {
    fn is_undefined(&self) -> bool {
        self.is_null()
    }

    fn is_array(&self) -> bool {
        self.is_array()
    }

    fn array_length(&self) -> usize {
        self.as_array().map_or(0, <[_]>::len)
    }

    fn array_member(&self, i: usize) -> Convertible<'_> {
        Convertible::new(self.as_array().and_then(|array| array.get(i)).unwrap_or(&NULL))
    }

    fn is_object(&self) -> bool {
        self.is_object()
    }

    fn object_member(&self, name: &str) -> Option<Convertible<'_>> {
        member(self.as_object()?, name).map(|value| Convertible::new(value))
    }

    fn each_member(&self, f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
        members(self.as_object()?).find_map(|(name, value)| f(name, &Convertible::new(value)))
    }

    fn to_bool(&self) -> Option<bool> {
        self.as_bool()
    }

    fn to_number(&self) -> Option<f32> {
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
            // As the serde_json front end: null converts to `false`.
            Json::Null => Some(Value::Bool(false)),
            Json::Bool(b) => Some(Value::Bool(*b)),
            Json::Str(s) => Some(Value::String(s.to_string())),
            Json::Number(_) => Some(number_value(self)),
            Json::Array(_) | Json::Object(_) => None,
        }
    }

    /// The `serde_json` front end's conversion: `serde_json_borrow` can't
    /// deserialize the enums of the `geojson` crate's types.
    fn to_geojson(&self, error: &mut Error) -> Option<GeoJson> {
        crate::json_conversion::to_geojson(&serde_json::Value::from(self), error)
    }
}

/// A JSON number's value: the first of an unsigned integer, a signed one and
/// a double that holds it, as in the `serde_json` front end.
fn number_value(json: &Json<'_>) -> Value {
    match (json.as_u64(), json.as_i64()) {
        (Some(u), _) => Value::Uint(u),
        (None, Some(i)) => Value::Int(i),
        (None, None) => Value::Double(json.as_f64().unwrap_or_default()),
    }
}

/// The value of a JSON value, arrays and objects included, as
/// [`json_to_value`](crate::json_to_value) reads `serde_json`'s.
#[must_use]
pub fn borrowed_json_to_value(json: &Json<'_>) -> Value {
    match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Str(s) => Value::String(s.to_string()),
        Json::Number(_) => number_value(json),
        Json::Array(array) => Value::Array(array.iter().map(borrowed_json_to_value).collect()),
        Json::Object(object) => Value::Object(
            members(object).map(|(key, member)| (key.to_owned(), borrowed_json_to_value(member))).collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Json<'_> {
        serde_json::from_str(text).unwrap()
    }

    /// Each accessor reads as it reads the same text through `serde_json`.
    #[test]
    fn reads_as_serde_json() {
        let text = r#"{"a": [1, -2, 3.5, "s", "e\"s", true, null, -0, 18446744073709551615, -9223372036854775808, 1e-320],
                       "b": {"c": 1}}"#;
        let (json, borrowed) = (serde_json::from_str::<serde_json::Value>(text).unwrap(), parse(text));
        let (j, b) = (Convertible::from(&json), Convertible::from(&borrowed));
        assert!(b.is_object() && !b.is_array());
        assert!(b.object_member("missing").is_none());
        let (ja, ba) = (j.object_member("a").unwrap(), b.object_member("a").unwrap());
        assert_eq!(ja.array_length(), ba.array_length());
        for i in 0..=ja.array_length() {
            let (jm, bm) = (ja.array_member(i), ba.array_member(i));
            assert_eq!(jm.is_undefined(), bm.is_undefined(), "{i}");
            assert_eq!(jm.to_value(), bm.to_value(), "{i}");
            assert_eq!(jm.to_number().map(f32::to_bits), bm.to_number().map(f32::to_bits), "{i}");
            assert_eq!(jm.to_double().map(f64::to_bits), bm.to_double().map(f64::to_bits), "{i}");
            assert_eq!(jm.to_string(), bm.to_string(), "{i}");
            assert_eq!(jm.to_bool(), bm.to_bool(), "{i}");
        }
        assert_eq!(crate::json_to_value(&json), borrowed_json_to_value(&borrowed));
    }

    /// A key named twice reads once, at its first position, with its last
    /// value, as `serde_json`'s map reads it.
    #[test]
    fn duplicate_keys_read_as_serde_json() {
        for count in [3, 20] {
            let fillers: Vec<_> = (0..count).map(|i| format!("\"k{i}\": {i}")).collect();
            let text = format!("{{\"a\": 1, \"b\": 2, {}, \"a\": 3}}", fillers.join(", "));
            let (json, borrowed) = (serde_json::from_str::<serde_json::Value>(&text).unwrap(), parse(&text));
            let mut names = Vec::new();
            Convertible::from(&borrowed).each_member(|name, value| {
                names.push((name.to_owned(), value.to_value()));
                None
            });
            let expected: Vec<_> = json
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, value)| (name.clone(), Convertible::from(value).to_value()))
                .collect();
            assert_eq!(names, expected);
            assert_eq!(Convertible::from(&borrowed).object_member("a").unwrap().to_value(), Some(Value::Uint(3)));
            assert_eq!(crate::json_to_value(&json), borrowed_json_to_value(&borrowed));
        }
    }

    #[test]
    fn each_member_short_circuits() {
        let json = parse(r#"{"a": 1, "b": 2, "c": 3}"#);
        let mut seen = 0;
        let result = Convertible::from(&json).each_member(|name, _| {
            seen += 1;
            (name == "b").then(|| Error::new("stop"))
        });
        assert_eq!(result, Some(Error::new("stop")));
        assert_eq!(seen, 2);
    }

    #[test]
    fn strings_borrow_from_the_text() {
        let json = parse(r#"{"key": "value", "escaped": "a\"b"}"#);
        let object = json.as_object().unwrap();
        assert!(matches!(member(object, "key"), Some(Json::Str(std::borrow::Cow::Borrowed("value")))));
        assert_eq!(member(object, "escaped").and_then(Json::as_str), Some("a\"b"));
    }

    #[test]
    fn reads_geojson() {
        let text = r#"{"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 1, "properties": {"a": [1, {"b": null}]},
             "geometry": {"type": "Point", "coordinates": [1, 2]}}]}"#;
        let mut error = Error::default();
        let borrowed =
            Convertible::from(&parse(text)).to_geojson(&mut error).unwrap_or_else(|| panic!("{}", error.message));
        assert_eq!(borrowed, text.parse::<GeoJson>().unwrap());

        let mut error = Error::default();
        assert!(Convertible::from(&parse(r#"{"type": "Pointy"}"#)).to_geojson(&mut error).is_none());
        assert_ne!(error.message, "");
    }
}
