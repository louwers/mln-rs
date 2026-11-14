//! A JSON document parsed for one conversion and dropped after it.
//!
//! The style parser reads a document once, by key and in document order, and
//! then drops it. `serde_json::Value` with `preserve_order` allocates a string
//! per key and string and an index per object for that. A [`JsonValue`]
//! borrows its strings from the text where it can and keeps each object's
//! members in a plain list. It parses with `serde_json`, so it accepts the
//! same documents and fails with the same errors.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use feature::Value;
use geojson::GeoJson;
use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Error as JsonError, Number};

use crate::conversion::{ConversionTraits, Convertible, Error};
use crate::json_conversion;

/// A JSON value whose strings borrow from the text it was parsed from.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum JsonValue<'a> {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(Cow<'a, str>),
    Array(Vec<JsonValue<'a>>),
    Object(JsonObject<'a>),
}

/// An object's members in document order. A key that appears twice keeps
/// its first position and its last value, as in `serde_json`'s
/// `preserve_order` map. A lookup is a linear search: the style parser looks
/// up a few names per object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonObject<'a> {
    members: Vec<(Cow<'a, str>, JsonValue<'a>)>,
}

static NULL: JsonValue<'static> = JsonValue::Null;

impl<'a> JsonValue<'a> {
    /// Parses `json`, failing with `serde_json`'s error.
    pub fn parse(json: &'a str) -> Result<Self, JsonError> {
        serde_json::from_str(json)
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, JsonValue::Null)
    }

    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self, JsonValue::Number(_))
    }

    #[must_use]
    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[JsonValue<'a>]> {
        match self {
            JsonValue::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&JsonObject<'a>> {
        match self {
            JsonValue::Object(o) => Some(o),
            _ => None,
        }
    }

    /// The member `name` of an object; `None` for a missing member or a
    /// value that isn't an object.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&JsonValue<'a>> {
        self.as_object()?.get(name)
    }

    /// The value as a `Value`, arrays and objects included. A number is the
    /// first of an unsigned integer, a signed one and a double that holds it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(b) => Value::Bool(*b),
            JsonValue::String(s) => Value::String(s.as_ref().to_owned()),
            JsonValue::Number(n) => number_to_value(n),
            JsonValue::Array(array) => Value::Array(array.iter().map(JsonValue::to_value).collect()),
            JsonValue::Object(object) => {
                Value::Object(object.iter().map(|(key, member)| (key.to_owned(), member.to_value())).collect())
            }
        }
    }

    /// The value as a `serde_json::Value`.
    #[must_use]
    pub fn to_serde_json(&self) -> serde_json::Value {
        match self {
            JsonValue::Null => serde_json::Value::Null,
            JsonValue::Bool(b) => serde_json::Value::Bool(*b),
            JsonValue::Number(n) => serde_json::Value::Number(n.clone()),
            JsonValue::String(s) => serde_json::Value::String(s.as_ref().to_owned()),
            JsonValue::Array(array) => serde_json::Value::Array(array.iter().map(JsonValue::to_serde_json).collect()),
            JsonValue::Object(object) => serde_json::Value::Object(
                object.iter().map(|(key, member)| (key.to_owned(), member.to_serde_json())).collect(),
            ),
        }
    }
}

fn number_to_value(n: &Number) -> Value {
    match (n.as_u64(), n.as_i64()) {
        (Some(u), _) => Value::Uint(u),
        (None, Some(i)) => Value::Int(i),
        (None, None) => Value::Double(n.as_f64().unwrap_or_default()),
    }
}

impl<'a> JsonObject<'a> {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&JsonValue<'a>> {
        self.members.iter().find(|(key, _)| key == name).map(|(_, value)| value)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The members in document order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, &JsonValue<'a>)> {
        self.members.iter().map(|(key, value)| (key.as_ref(), value))
    }

    /// Merges repeated keys: each keeps its first position and takes its
    /// last value.
    fn merge_repeated_keys(members: Vec<(Cow<'a, str>, JsonValue<'a>)>) -> Vec<(Cow<'a, str>, JsonValue<'a>)> {
        // Below this, comparing every pair is cheaper than hashing.
        const SMALL: usize = 16;
        let mut positions: HashMap<&str, usize> = HashMap::new();
        let repeated = if members.len() <= SMALL {
            members.iter().enumerate().any(|(i, (key, _))| members[..i].iter().any(|(k, _)| k == key))
        } else {
            positions.reserve(members.len());
            members.iter().enumerate().any(|(i, (key, _))| positions.insert(key.as_ref(), i).is_some())
        };
        if !repeated {
            return members;
        }
        // The index of the first member with each member's key.
        positions.clear();
        let first: Vec<usize> =
            members.iter().enumerate().map(|(i, (key, _))| *positions.entry(key.as_ref()).or_insert(i)).collect();
        drop(positions);
        let mut merged: Vec<Option<(Cow<'a, str>, JsonValue<'a>)>> = Vec::with_capacity(members.len());
        for ((key, value), at) in members.into_iter().zip(first) {
            if at == merged.len() {
                merged.push(Some((key, value)));
            } else {
                if let Some(Some(member)) = merged.get_mut(at) {
                    member.1 = value;
                }
                merged.push(None);
            }
        }
        merged.into_iter().flatten().collect()
    }
}

impl<'de> Deserialize<'de> for JsonValue<'de> {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(JsonValue::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(JsonValue::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(Number::from_f64(value).map_or(JsonValue::Null, JsonValue::Number))
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(JsonValue::String(Cow::Borrowed(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(JsonValue::String(Cow::Owned(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(JsonValue::String(Cow::Owned(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        Deserialize::deserialize(deserializer)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut array = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(element) = seq.next_element()? {
            array.push(element);
        }
        Ok(JsonValue::Array(array))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some(Key(key)) = map.next_key()? {
            members.push((key, map.next_value()?));
        }
        Ok(JsonValue::Object(JsonObject { members: JsonObject::merge_repeated_keys(members) }))
    }
}

/// An object key, borrowed from the text unless it has escapes (`Cow`'s own
/// `Deserialize` always copies).
struct Key<'de>(Cow<'de, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = Key<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string key")
            }

            fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(Key(Cow::Borrowed(value)))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Key(Cow::Owned(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(Key(Cow::Owned(value)))
            }
        }

        deserializer.deserialize_str(KeyVisitor)
    }
}

impl ConversionTraits for JsonValue<'_> {
    fn is_undefined(&self) -> bool {
        self.is_null()
    }

    fn is_array(&self) -> bool {
        matches!(self, JsonValue::Array(_))
    }

    fn array_length(&self) -> usize {
        self.as_array().map_or(0, <[_]>::len)
    }

    fn array_member(&self, i: usize) -> Convertible<'_> {
        Convertible::new(self.as_array().and_then(|a| a.get(i)).unwrap_or(&NULL))
    }

    fn is_object(&self) -> bool {
        self.is_object()
    }

    fn object_member(&self, name: &str) -> Option<Convertible<'_>> {
        self.get(name).map(|m| Convertible::new(m))
    }

    fn each_member(&self, f: &mut dyn FnMut(&str, &Convertible<'_>) -> Option<Error>) -> Option<Error> {
        let JsonValue::Object(o) = self else {
            return None;
        };
        for (name, value) in o.iter() {
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
            JsonValue::Null | JsonValue::Bool(false) => Some(Value::Bool(false)),
            JsonValue::Bool(true) => Some(Value::Bool(true)),
            JsonValue::String(s) => Some(Value::String(s.as_ref().to_owned())),
            JsonValue::Number(n) => Some(number_to_value(n)),
            JsonValue::Array(_) | JsonValue::Object(_) => None,
        }
    }

    /// Reads the GeoJSON from a `serde_json::Value` copy: inline GeoJSON is
    /// a small part of a style, and one source type keeps one copy of the
    /// `geojson` crate's deserializer in the browser build.
    fn to_geojson(&self, error: &mut Error) -> Option<GeoJson> {
        json_conversion::to_geojson(&self.to_serde_json(), error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses as `serde_json::Value` does: the same members in the same
    /// order, the same numbers, and the same errors.
    #[test]
    fn parses_as_serde_json_does() {
        let json = r#"{"b": [1, -2, 3.5, -0, 1e300, 18446744073709551616, "s\u00e9", true, null],
            "a": {"c": 1, "d": 2, "c": 3}, "e\n": "x"}"#;
        let ours = JsonValue::parse(json).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(ours.to_serde_json(), theirs);
        let keys: Vec<_> = ours.get("a").unwrap().as_object().unwrap().iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["c", "d"]);
        assert_eq!(ours.get("a").unwrap().get("c").unwrap().as_i64(), Some(3));
        let top: Vec<_> = ours.as_object().unwrap().iter().map(|(k, _)| k).collect();
        assert_eq!(top, ["b", "a", "e\n"]);
        assert!(matches!(ours.get("e\n"), Some(JsonValue::String(Cow::Borrowed("x")))));

        for bad in ["", "{", "[1,]", r#"{"a" 1}"#, "1 2", "\"\\x\"", "1e400"] {
            let ours = JsonValue::parse(bad).unwrap_err().to_string();
            let theirs = serde_json::from_str::<serde_json::Value>(bad).unwrap_err().to_string();
            assert_eq!(ours, theirs, "{bad}");
        }
    }

    /// Repeated keys in a large object merge as in a small one.
    #[test]
    fn merges_repeated_keys_of_a_large_object() {
        let mut members: Vec<String> = (0..40).map(|i| format!(r#""k{i}": {i}"#)).collect();
        members.insert(30, r#""k3": "again""#.to_owned());
        members.push(r#""k39": "last""#.to_owned());
        let json = format!("{{{}}}", members.join(","));
        let ours = JsonValue::parse(&json).unwrap();
        assert_eq!(ours.to_serde_json(), serde_json::from_str::<serde_json::Value>(&json).unwrap());
        let object = ours.as_object().unwrap();
        assert_eq!(object.len(), 40);
        assert_eq!(object.iter().nth(3).unwrap(), ("k3", &JsonValue::String("again".into())));
        assert_eq!(object.iter().last().unwrap(), ("k39", &JsonValue::String("last".into())));
    }

    /// Converts as the `serde_json::Value` front end does.
    #[test]
    fn converts_as_serde_json_does() {
        let json = r#"{"a": [1, -2, 3.5, "s", true, null], "b": {"c": 1}}"#;
        let ours = JsonValue::parse(json).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(json).unwrap();
        let (ours, theirs) = (Convertible::from(&ours), Convertible::from(&theirs));
        let (a, b) = (ours.object_member("a").unwrap(), theirs.object_member("a").unwrap());
        assert_eq!(a.array_length(), b.array_length());
        for i in 0..7 {
            let (x, y) = (a.array_member(i), b.array_member(i));
            assert_eq!(x.to_value(), y.to_value());
            assert_eq!(x.to_number(), y.to_number());
            assert_eq!(x.to_string(), y.to_string());
            assert_eq!(x.to_bool(), y.to_bool());
            assert_eq!(x.is_undefined(), y.is_undefined());
        }
        assert!(ours.object_member("missing").is_none());
        let mut members = Vec::new();
        ours.each_member(|name, _| {
            members.push(name.to_owned());
            None
        });
        assert_eq!(members, ["a", "b"]);
        assert_eq!(
            JsonValue::parse(json).unwrap().to_value(),
            crate::json_to_value(&serde_json::from_str(json).unwrap())
        );
    }

    /// Reads GeoJSON, and fails to, as the `serde_json::Value` front end does.
    #[test]
    fn reads_geojson_as_serde_json_does() {
        for json in [
            r#"{"type":"FeatureCollection","features":[{"type":"Feature","id":7,"properties":{"a":[1,{"b":null}]},
                "geometry":{"type":"Point","coordinates":[1,2.5]}}],"foreign":{"x":1}}"#,
            r#"{"type":"Nope"}"#,
            r#"{"type":"Point","coordinates":"x"}"#,
        ] {
            let ours = JsonValue::parse(json).unwrap();
            let theirs: serde_json::Value = serde_json::from_str(json).unwrap();
            let (mut ours_error, mut theirs_error) = (Error::default(), Error::default());
            assert_eq!(
                Convertible::from(&ours).to_geojson(&mut ours_error),
                Convertible::from(&theirs).to_geojson(&mut theirs_error),
                "{json}"
            );
            assert_eq!(ours_error, theirs_error, "{json}");
        }
    }
}
