// Ports include/rapidjson/writer.h (RapidJSON, (C) 2015 THL A29 Limited, a
// Tencent company, and Milo Yip, MIT License) for the instantiation MapLibre
// Native uses: `Writer<StringBuffer>` = UTF-8 in, UTF-8 out, default flags
// (`kWriteNoFlags`: no encoding validation, NaN/Inf rejected), default
// `maxDecimalPlaces` 324.
//
// `RAPIDJSON_ASSERT` is `assert`, live in the C++ debug build; the
// equivalents here are `debug_assert!`.

use crate::internal::dtoa::{DEFAULT_MAX_DECIMAL_PLACES, dtoa};
use crate::internal::ieee754::Double;
use crate::internal::itoa::{i32toa, i64toa, u32toa, u64toa};

/// `rapidjson::Type` (only used for the Prefix assertions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Null,
    False,
    True,
    Object,
    Array,
    String,
    Number,
}

/// Information for each nested level.
#[derive(Clone, Copy, Debug)]
struct Level {
    /// number of values in this level
    value_count: usize,
    /// true if in array, otherwise in object
    in_array: bool,
}

const HEX_DIGITS: [u8; 16] = *b"0123456789ABCDEF";

/// The escape table of `WriteString` for bytes 0x00..=0x7F (bytes 0x80 and
/// up are copied unchanged).
const fn escape(c: u8) -> u8 {
    match c {
        0x08 => b'b',
        0x09 => b't',
        0x0A => b'n',
        0x0C => b'f',
        0x0D => b'r',
        0x00..=0x1F => b'u',
        b'"' => b'"',
        b'\\' => b'\\',
        _ => 0,
    }
}

/// `rapidjson::Writer<rapidjson::StringBuffer>`.
#[derive(Debug)]
pub struct Writer {
    os: String,
    level_stack: Vec<Level>,
    max_decimal_places: i32,
    has_root: bool,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    #[must_use]
    pub fn new() -> Self {
        Writer {
            os: String::new(),
            level_stack: Vec::new(),
            max_decimal_places: DEFAULT_MAX_DECIMAL_PLACES,
            has_root: false,
        }
    }

    /// `StringBuffer::GetString()`.
    #[must_use]
    pub fn get_string(&self) -> &str {
        &self.os
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.os
    }

    #[must_use]
    pub fn get_max_decimal_places(&self) -> i32 {
        self.max_decimal_places
    }

    pub fn set_max_decimal_places(&mut self, max_decimal_places: i32) {
        self.max_decimal_places = max_decimal_places;
    }

    /// Checks whether the output is a complete JSON.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.has_root && self.level_stack.is_empty()
    }

    pub fn null(&mut self) -> bool {
        self.prefix(Type::Null);
        self.os.push_str("null");
        true
    }

    pub fn bool_(&mut self, b: bool) -> bool {
        self.prefix(if b { Type::True } else { Type::False });
        self.os.push_str(if b { "true" } else { "false" });
        true
    }

    pub fn int(&mut self, i: i32) -> bool {
        self.prefix(Type::Number);
        self.os.push_str(&i32toa(i));
        true
    }

    pub fn uint(&mut self, u: u32) -> bool {
        self.prefix(Type::Number);
        self.os.push_str(&u32toa(u));
        true
    }

    pub fn int64(&mut self, i64_: i64) -> bool {
        self.prefix(Type::Number);
        self.os.push_str(&i64toa(i64_));
        true
    }

    pub fn uint64(&mut self, u64_: u64) -> bool {
        self.prefix(Type::Number);
        self.os.push_str(&u64toa(u64_));
        true
    }

    /// Writes the given double. NaN and infinities write nothing and return
    /// false (the separator from `Prefix` is still written).
    pub fn double(&mut self, d: f64) -> bool {
        self.prefix(Type::Number);
        self.write_double(d)
    }

    pub fn raw_number(&mut self, s: &str) -> bool {
        self.prefix(Type::Number);
        self.write_string(s)
    }

    pub fn string(&mut self, s: &str) -> bool {
        self.prefix(Type::String);
        self.write_string(s)
    }

    pub fn start_object(&mut self) -> bool {
        self.prefix(Type::Object);
        self.level_stack.push(Level { value_count: 0, in_array: false });
        self.os.push('{');
        true
    }

    pub fn key(&mut self, s: &str) -> bool {
        self.string(s)
    }

    pub fn end_object(&mut self) -> bool {
        debug_assert!(!self.level_stack.is_empty()); // not inside an Object
        debug_assert!(!self.level_stack.last().unwrap().in_array); // currently inside an Array, not Object
        debug_assert!(self.level_stack.last().unwrap().value_count.is_multiple_of(2)); // Object has a Key without a Value
        self.level_stack.pop();
        self.os.push('}');
        true
    }

    pub fn start_array(&mut self) -> bool {
        self.prefix(Type::Array);
        self.level_stack.push(Level { value_count: 0, in_array: true });
        self.os.push('[');
        true
    }

    pub fn end_array(&mut self) -> bool {
        debug_assert!(!self.level_stack.is_empty());
        debug_assert!(self.level_stack.last().unwrap().in_array);
        self.level_stack.pop();
        self.os.push(']');
        true
    }

    /// Writes a raw JSON value (`RawValue`).
    pub fn raw_value(&mut self, json: &str, type_: Type) -> bool {
        self.prefix(type_);
        self.os.push_str(json);
        true
    }

    fn write_double(&mut self, d: f64) -> bool {
        if Double::from_f64(d).is_nan_or_inf() {
            // kWriteNanAndInfFlag is not set.
            return false;
        }
        self.os.push_str(&dtoa(d, self.max_decimal_places));
        true
    }

    fn write_string(&mut self, s: &str) -> bool {
        self.os.push('"');
        // The C++ loops over bytes; only bytes < 0x80 can be escaped, so
        // looping over chars and copying non-ASCII chars whole (what
        // TranscodeUnsafe UTF-8 -> UTF-8 does byte by byte) is equivalent.
        for ch in s.chars() {
            let e = if ch.is_ascii() { escape(ch as u8) } else { 0 };
            if e != 0 {
                let c = ch as u8;
                self.os.push('\\');
                self.os.push(e as char);
                if e == b'u' {
                    self.os.push('0');
                    self.os.push('0');
                    self.os.push(HEX_DIGITS[(c >> 4) as usize] as char);
                    self.os.push(HEX_DIGITS[(c & 0xF) as usize] as char);
                }
            } else {
                self.os.push(ch);
            }
        }
        self.os.push('"');
        true
    }

    fn prefix(&mut self, type_: Type) {
        let _ = type_;
        if let Some(level) = self.level_stack.last_mut() {
            // this value is not at root
            if level.value_count > 0 {
                if level.in_array {
                    self.os.push(','); // add comma if it is not the first element in array
                } else {
                    // in object
                    self.os.push(if level.value_count % 2 == 0 { ',' } else { ':' });
                }
            }
            if !level.in_array && level.value_count % 2 == 0 {
                debug_assert_eq!(type_, Type::String); // if it's in object, then even number should be a name
            }
            level.value_count += 1;
        } else {
            debug_assert!(!self.has_root); // Should only has one and only one root.
            self.has_root = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested() {
        let mut w = Writer::new();
        w.start_object();
        w.key("a");
        w.start_array();
        w.int(1);
        w.double(f64::NAN);
        w.double(2.5);
        w.null();
        w.bool_(true);
        w.end_array();
        w.key("b\n\"\\\u{1}\u{7f}é");
        w.uint64(u64::MAX);
        w.end_object();
        assert!(w.is_complete());
        assert_eq!(w.get_string(), "{\"a\":[1,,2.5,null,true],\"b\\n\\\"\\\\\\u0001\u{7f}é\":18446744073709551615}");
    }

    #[test]
    fn root_double() {
        let mut w = Writer::new();
        assert!(!w.double(f64::INFINITY));
        assert_eq!(w.get_string(), "");
        let mut w = Writer::new();
        assert!(w.double(-0.0));
        assert_eq!(w.into_string(), "-0.0");
    }
}
