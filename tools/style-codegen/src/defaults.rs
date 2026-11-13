// The JS emits a C++ initializer (`0.85f`, `{{0.f, 0.f}}`, `Color::black()`,
// `{}` ...). Here `default_value` builds the same initializer as a small
// tree (`Init`), and `rust_value` prints the Rust expression for the value
// the C++ compiler gives that initializer for the property's C++ type: the
// same literal parsing (`0.85f` is the decimal parsed straight to float),
// the same braced-init rules (`{}` value-initializes, a short `std::array`
// initializer zero-fills the rest), the same constructor picks
// (`Padding{2}` is the one-value constructor). Initializers that would not
// compile in C++ are generator errors.

use serde_json::Value;

use crate::spec::Property;
use crate::types::{EvaluatedType, camelize, evaluated_type, evaluated_type_unnamed};

/// A C++ initializer as `defaultValue` writes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Init {
    /// A float literal: the decimal text before the `f` suffix
    /// (`formatNumber`).
    FloatLiteral(String),
    /// A JS value written verbatim (`return property.default`): an integer
    /// literal or `true`/`false`.
    Raw(Value),
    /// `{}`.
    Empty,
    /// `{a, b, ...}`.
    Braced(Vec<Init>),
    /// A string literal (`JSON.stringify(property.default)`).
    StringLiteral(String),
    Enumerator(String, String),
    ColorBlack,
    ColorWhite,
    /// `{ r, g, b, a }` / `Color{r, g, b, a}` with the JS double values.
    ColorComponents([f64; 4]),
}

impl Init {
    /// The C++ spelling, as the template writes it (for error messages).
    pub(crate) fn cpp(&self) -> String {
        match self {
            Init::FloatLiteral(text) => format!("{text}{}f", if text.contains('.') { "" } else { "." }),
            Init::Raw(value) => value.to_string(),
            Init::Empty => "{}".into(),
            Init::Braced(items) => format!("{{{}}}", items.iter().map(Init::cpp).collect::<Vec<_>>().join(", ")),
            Init::StringLiteral(s) => serde_json::to_string(s).unwrap(),
            Init::Enumerator(t, v) => format!("{t}::{v}"),
            Init::ColorBlack => "Color::black()".into(),
            Init::ColorWhite => "Color::white()".into(),
            Init::ColorComponents(c) => {
                format!("{{ {} }}", c.iter().map(|v| js_number(*v)).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// Formats `value` as JS `Number.prototype.toString()` does (shortest
/// round-trip digits, JS exponent rules), for the doubles the JS prints.
#[must_use]
pub(crate) fn js_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` prints the shortest round-trip digits: "d.ddde<exp>".
    let formatted = format!("{:e}", value.abs());
    let (mantissa, exponent) = formatted.split_once('e').expect("exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exponent: i32 = exponent.parse().expect("exponent");
    let k = digits.len() as i32;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e >= 0 { format!("+{e}") } else { e.to_string() };
        if k == 1 { format!("{digits}e{e}") } else { format!("{}.{}e{e}", &digits[..1], &digits[1..]) }
    };
    format!("{sign}{body}")
}

/// Formats a `property.default` that is a JS number as JS `num.toString()`
/// does.
fn js_to_string(value: &Value) -> String {
    match value {
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => js_number(i as f64),
            (_, Some(u)) => js_number(u as f64),
            _ => js_number(n.as_f64().expect("number")),
        },
        other => panic!("formatNumber of {other}"),
    }
}

/// A number default (0 when `num` is `None`): a float literal when the
/// evaluated type is float or the property type is `number`, else the raw
/// value.
fn format_number(evaluated: &EvaluatedType, property_type: &str, num: Option<&Value>) -> Init {
    let num = num.cloned().unwrap_or(Value::from(0));
    if *evaluated == EvaluatedType::Float || property_type == "number" {
        Init::FloatLiteral(js_to_string(&num))
    } else {
        Init::Raw(num)
    }
}

/// Parses a CSS color into the csscolorparser crate's bytes, premultiplied in
/// JS doubles.
fn parse_css_color(s: &str) -> [f64; 4] {
    let color = csscolorparser::parse(s).unwrap_or_else(|_| panic!("Could not parse CSS color {s:?}"));
    // The JS parser keeps the alpha as a double; the crate keeps a float.
    // They agree for alpha 0 and 1, the only alphas of the spec defaults;
    // refuse others rather than guess.
    let a = f64::from(color.a);
    assert!(a == 0.0 || a == 1.0, "CSS color default {s:?} with a fractional alpha");
    let [r, g, b, _] = color.to_rgba8();
    [f64::from(r) / 255.0 * a, f64::from(g) / 255.0 * a, f64::from(b) / 255.0 * a, a]
}

fn color_init(s: &str, in_array: bool) -> Init {
    let color = parse_css_color(s);
    match color {
        [0.0, 0.0, 0.0, 0.0] => {
            if in_array {
                Init::Braced(vec![Init::Empty])
            } else {
                Init::Empty
            }
        }
        [0.0, 0.0, 0.0, 1.0] => Init::ColorBlack,
        [1.0, 1.0, 1.0, 1.0] => Init::ColorWhite,
        c => Init::ColorComponents(c),
    }
}

/// The JS truthiness of `property.default`.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(_) => true,
    }
}

/// The parts of a property `defaultValue` reads; array elements are
/// `{type: property.value, default: e}`.
struct DefaultQuery<'a> {
    property: Option<&'a Property>,
    type_: Option<&'a str>,
    value: Option<&'a str>,
    length: Option<usize>,
    default: Option<&'a Value>,
}

fn default_of(q: &DefaultQuery<'_>) -> Init {
    let name = q.property.map(|p| p.name.as_str());
    let evaluated = || match q.property {
        Some(p) => evaluated_type(p),
        None => evaluated_type_unnamed(q.type_.expect("type")),
    };

    // https://github.com/mapbox/mapbox-gl-native/issues/5258
    if name == Some("line-round-limit") {
        return Init::Raw(Value::from(1));
    }

    if name == Some("fill-outline-color") {
        return Init::Empty;
    }

    if q.property.and_then(Property::property_type) == Some("color-ramp") {
        return Init::Empty;
    }

    match q.type_ {
        Some("number") => format_number(&evaluated(), "number", q.default),
        Some("numberArray") => {
            // Default is a single number, wrap it in a vector
            Init::Braced(vec![format_number(&EvaluatedType::Float, "number", q.default)])
        }
        Some("colorArray") => {
            // Default is a single color string, parse it and wrap in a vector
            let s = q.default.and_then(Value::as_str).expect("colorArray default");
            match color_init(s, true) {
                Init::ColorBlack => Init::Braced(vec![Init::ColorBlack]),
                Init::ColorWhite => Init::Braced(vec![Init::ColorWhite]),
                Init::ColorComponents(c) => Init::Braced(vec![Init::Braced(vec![Init::ColorComponents(c)])]),
                other => other,
            }
        }
        Some("formatted" | "string" | "resolvedImage" | "variableAnchorOffsetCollection") => {
            if truthy(q.default) {
                match q.default {
                    Some(Value::String(s)) => Init::Braced(vec![Init::StringLiteral(s.clone())]),
                    Some(other) => panic!("{name:?}: non-string default {other}"),
                    None => unreachable!(),
                }
            } else {
                Init::Empty
            }
        }
        Some("enum") => {
            let t = match evaluated() {
                EvaluatedType::Enum(t) => t,
                other => panic!("{name:?}: enum with evaluated type {other:?}"),
            };
            match q.default {
                None => Init::Enumerator(t, "Undefined".into()),
                Some(Value::String(s)) => Init::Enumerator(t, camelize(s)),
                Some(other) => panic!("{name:?}: enum default {other}"),
            }
        }
        Some("color") => {
            let s = q.default.and_then(Value::as_str).unwrap_or_else(|| panic!("{name:?}: color default"));
            color_init(s, false)
        }
        Some("array" | "padding") => {
            let defaults: Vec<Init> = match q.default {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|e| {
                        default_of(&DefaultQuery {
                            property: None,
                            type_: q.value,
                            value: None,
                            length: None,
                            default: Some(e),
                        })
                    })
                    .collect(),
                Some(other) => panic!("{name:?}: array default {other}"),
            };
            if q.length.is_some() { Init::Braced(vec![Init::Braced(defaults)]) } else { Init::Braced(defaults) }
        }
        _ => Init::Raw(q.default.cloned().unwrap_or(Value::Null)),
    }
}

#[must_use]
pub(crate) fn default_value(property: &Property) -> Init {
    default_of(&DefaultQuery {
        property: Some(property),
        type_: Some(property.type_()),
        value: property.value(),
        length: property.length(),
        default: property.default(),
    })
}

/// A Rust f32 literal that denotes exactly `value`.
fn f32_literal(value: f32) -> String {
    assert!(value.is_finite(), "non-finite default {value}");
    format!("{value:?}_f32")
}

fn f64_literal(value: f64) -> String {
    assert!(value.is_finite(), "non-finite default {value}");
    format!("{value:?}_f64")
}

/// The value of a scalar initializer converted to float, as C++ does for a
/// `float` target (`0.85f`: decimal to float; `1`: int to float).
fn as_f32(init: &Init) -> f32 {
    match init {
        Init::FloatLiteral(text) => text.parse::<f32>().unwrap_or_else(|_| panic!("float literal {text}")),
        Init::Raw(Value::Number(n)) => match n.as_i64() {
            Some(i) => i as f32,
            None => panic!("non-integer raw number {n} for a float"),
        },
        other => panic!("{} is not a float initializer", other.cpp()),
    }
}

/// The value for a `double` target: a float literal is a float first.
fn as_f64(init: &Init) -> f64 {
    match init {
        Init::FloatLiteral(_) => f64::from(as_f32(init)),
        Init::Raw(Value::Number(n)) => match n.as_i64() {
            Some(i) => i as f64,
            None => n.as_f64().expect("number"),
        },
        other => panic!("{} is not a double initializer", other.cpp()),
    }
}

/// The Rust expression for `T x = <init>;` (copy-initialization, as in
/// `static T defaultValue() { return <init>; }`).
#[must_use]
pub(crate) fn rust_value(init: &Init, t: &EvaluatedType) -> String {
    match (t, init) {
        (_, Init::Empty) => rust_value_initialized(t),
        (EvaluatedType::Bool, Init::Raw(Value::Bool(b))) => b.to_string(),
        (EvaluatedType::Float, _) => f32_literal(as_f32(init)),
        (EvaluatedType::Double, _) => f64_literal(as_f64(init)),
        // `Rotation(double angle_)` from a float.
        (EvaluatedType::Rotation, _) => format!("Rotation::new({})", f64_literal(as_f64(init))),
        (EvaluatedType::Enum(name), Init::Enumerator(t, v)) => {
            assert_eq!(name, t, "enumerator of another enum");
            assert_ne!(v, "Undefined", "{name} has no Undefined enumerator");
            format!("{name}::{v}")
        }
        (EvaluatedType::Color, Init::ColorBlack) => "Color::black()".into(),
        (EvaluatedType::Color, Init::ColorWhite) => "Color::white()".into(),
        (EvaluatedType::Color, Init::ColorComponents(c)) => format!(
            "Color::new({}, {}, {}, {})",
            f32_literal(c[0] as f32),
            f32_literal(c[1] as f32),
            f32_literal(c[2] as f32),
            f32_literal(c[3] as f32)
        ),
        // `Formatted(const char*)`, `Image(const char*)`, `std::string`.
        (EvaluatedType::Formatted, Init::Braced(items)) => match items.as_slice() {
            [Init::StringLiteral(s)] => format!("Formatted::from_plain({})", rust_string_literal(s)),
            _ => panic!("Formatted initializer {}", init.cpp()),
        },
        (EvaluatedType::Image, Init::Braced(items)) => match items.as_slice() {
            [Init::StringLiteral(s)] => format!("Image::new({})", rust_string_literal(s)),
            _ => panic!("Image initializer {}", init.cpp()),
        },
        (EvaluatedType::String, Init::Braced(items)) => match items.as_slice() {
            [Init::StringLiteral(s)] => format!("{}.to_owned()", rust_string_literal(s)),
            _ => panic!("std::string initializer {}", init.cpp()),
        },
        (EvaluatedType::String, Init::StringLiteral(s)) => format!("{}.to_owned()", rust_string_literal(s)),
        // `Padding{2}`: `Padding(float)`; `Padding{t, r, b, l}`: the
        // four-value constructor. Two or three values do not compile.
        (EvaluatedType::Padding, Init::Braced(items)) => match items.len() {
            1 => format!("Padding::uniform({})", f32_literal(as_f32(&items[0]))),
            4 => format!(
                "Padding::new({}, {}, {}, {})",
                f32_literal(as_f32(&items[0])),
                f32_literal(as_f32(&items[1])),
                f32_literal(as_f32(&items[2])),
                f32_literal(as_f32(&items[3]))
            ),
            n => panic!("Padding with {n} values does not compile in C++"),
        },
        // `std::array<T, N> x = {{a, b}}`: aggregate initialization; missing
        // elements are value-initialized.
        (EvaluatedType::Array(element, n), Init::Braced(outer)) => {
            let items = match outer.as_slice() {
                [Init::Braced(items)] => items.clone(),
                // Brace elision: `{a, b}` initializes the inner array too.
                items => items.to_vec(),
            };
            assert!(items.len() <= *n, "too many initializers for std::array<_, {n}>");
            let mut values: Vec<String> = items.iter().map(|item| rust_value(item, element)).collect();
            while values.len() < *n {
                values.push(rust_value_initialized(element));
            }
            format!("[{}]", values.join(", "))
        }
        // `std::vector<T> x = {a, b}`: the initializer-list constructor.
        (EvaluatedType::Vector(element), Init::Braced(items)) => {
            if items.is_empty() {
                "Vec::new()".into()
            } else {
                format!("vec![{}]", items.iter().map(|item| rust_value(item, element)).collect::<Vec<_>>().join(", "))
            }
        }
        // `{x}` for a scalar: copy-list-initialization from `x`.
        (_, Init::Braced(items)) if items.len() == 1 => rust_value(&items[0], t),
        _ => panic!("no C++ conversion from {} to {}", init.cpp(), t.cpp()),
    }
}

/// `T{}` (value-initialization).
fn rust_value_initialized(t: &EvaluatedType) -> String {
    match t {
        EvaluatedType::Bool => "false".into(),
        EvaluatedType::Float => "0.0_f32".into(),
        EvaluatedType::Double => "0.0_f64".into(),
        EvaluatedType::String => "String::new()".into(),
        EvaluatedType::Vector(_) => "Vec::new()".into(),
        EvaluatedType::Array(element, n) => {
            format!("[{}]", vec![rust_value_initialized(element); *n].join(", "))
        }
        other => format!("{}::default()", other.rust()),
    }
}

fn rust_string_literal(s: &str) -> String {
    assert!(
        s.chars().all(|c| (' '..='~').contains(&c) && c != '"' && c != '\\'),
        "string default {s:?} needs C/JSON escape handling"
    );
    format!("\"{s}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_number_formatting() {
        assert_eq!(js_number(0.1), "0.1");
        assert_eq!(js_number(1.0), "1");
        assert_eq!(js_number(250.0), "250");
        assert_eq!(js_number(1.15), "1.15");
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(js_number(1e20), "100000000000000000000");
        assert_eq!(js_number(1e-7), "1e-7");
        assert_eq!(js_number(0.000_001), "0.000001");
        assert_eq!(js_number(-2.5), "-2.5");
        assert_eq!(js_number(123.456e-10), "1.23456e-8");
    }

    #[test]
    fn float_literals_parse_the_decimal_text() {
        assert_eq!(rust_value(&Init::FloatLiteral("0.85".into()), &EvaluatedType::Float), "0.85_f32");
        assert_eq!(rust_value(&Init::Raw(Value::from(1)), &EvaluatedType::Float), "1.0_f32");
        assert_eq!(
            rust_value(&Init::FloatLiteral("1.15".into()), &EvaluatedType::Double),
            format!("{:?}_f64", f64::from(1.15_f32))
        );
    }

    #[test]
    fn short_array_initializers_zero_fill() {
        let init = Init::Braced(vec![Init::Braced(vec![Init::FloatLiteral("1".into())])]);
        let t = EvaluatedType::Array(Box::new(EvaluatedType::Float), 2);
        assert_eq!(rust_value(&init, &t), "[1.0_f32, 0.0_f32]");
    }
}
