use serde_json::Value;

use crate::spec::Property;

/// The camel-case spelling of a property name (`line-gap-width` ->
/// `LineGapWidth`).
#[must_use]
pub(crate) fn camelize(s: &str) -> String {
    let mut result = String::new();
    let mut upper_next = true;
    for c in s.chars() {
        if upper_next {
            result.extend(c.to_uppercase());
            upper_next = false;
        } else if c == '-' {
            upper_next = true;
        } else {
            result.push(c);
        }
    }
    result
}

/// The Rust spelling of a property name (`line-gap-width` -> `line_gap_width`).
#[must_use]
pub(crate) fn snake(s: &str) -> String {
    s.replace('-', "_")
}

#[must_use]
pub(crate) fn is_overridable(property: &Property) -> bool {
    ["text-color"].contains(&property.name.as_str())
}

/// The evaluated type of a property, as a type the generator can print both as
/// C++ (for error messages) and as Rust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EvaluatedType {
    Bool,
    Float,
    Double,
    String,
    Color,
    Padding,
    Image,
    Formatted,
    VariableAnchorOffsetCollection,
    Position,
    Rotation,
    /// A `style/types.hpp` enum, e.g. `LineCapType`.
    Enum(String),
    Array(Box<EvaluatedType>, usize),
    Vector(Box<EvaluatedType>),
}

impl EvaluatedType {
    #[must_use]
    pub(crate) fn cpp(&self) -> String {
        match self {
            EvaluatedType::Bool => "bool".into(),
            EvaluatedType::Float => "float".into(),
            EvaluatedType::Double => "double".into(),
            EvaluatedType::String => "std::string".into(),
            EvaluatedType::Color => "Color".into(),
            EvaluatedType::Padding => "Padding".into(),
            EvaluatedType::Image => "expression::Image".into(),
            EvaluatedType::Formatted => "expression::Formatted".into(),
            EvaluatedType::VariableAnchorOffsetCollection => "VariableAnchorOffsetCollection".into(),
            EvaluatedType::Position => "Position".into(),
            EvaluatedType::Rotation => "Rotation".into(),
            EvaluatedType::Enum(name) => name.clone(),
            EvaluatedType::Array(t, n) => format!("std::array<{}, {n}>", t.cpp()),
            EvaluatedType::Vector(t) => format!("std::vector<{}>", t.cpp()),
        }
    }

    #[must_use]
    pub(crate) fn rust(&self) -> String {
        match self {
            EvaluatedType::Bool => "bool".into(),
            EvaluatedType::Float => "f32".into(),
            EvaluatedType::Double => "f64".into(),
            EvaluatedType::String => "String".into(),
            EvaluatedType::Color => "Color".into(),
            EvaluatedType::Padding => "Padding".into(),
            EvaluatedType::Image => "Image".into(),
            EvaluatedType::Formatted => "Formatted".into(),
            EvaluatedType::VariableAnchorOffsetCollection => "VariableAnchorOffsetCollection".into(),
            EvaluatedType::Position => "Position".into(),
            EvaluatedType::Rotation => "Rotation".into(),
            EvaluatedType::Enum(name) => name.clone(),
            EvaluatedType::Array(t, n) => format!("[{}; {n}]", t.rust()),
            EvaluatedType::Vector(t) => format!("Vec<{}>", t.rust()),
        }
    }

    /// The named (non-primitive) Rust types this type mentions, for `use`
    /// lines.
    pub(crate) fn named_types(&self, out: &mut Vec<String>) {
        match self {
            EvaluatedType::Bool | EvaluatedType::Float | EvaluatedType::Double | EvaluatedType::String => {}
            EvaluatedType::Array(t, _) | EvaluatedType::Vector(t) => t.named_types(out),
            other => out.push(other.rust()),
        }
    }
}

/// The inputs `evaluatedType` reads: a whole property, or the `{type, name}`
/// objects the JS builds for array elements (whose other fields are
/// `undefined`).
struct TypeQuery<'a> {
    name: Option<&'a str>,
    type_: &'a str,
    value: Option<&'a str>,
    length: Option<usize>,
    period: Option<&'a Value>,
    units: Option<&'a str>,
    light: bool,
}

/// The string a JS regex tests for a missing name:
/// `RegExp.prototype.test(undefined)` tests the string "undefined".
fn js_name(name: Option<&str>) -> &str {
    name.unwrap_or("undefined")
}

fn evaluated_type_of(q: &TypeQuery<'_>) -> EvaluatedType {
    let name = js_name(q.name);
    if name.contains("text-variable-anchor-offset") {
        return EvaluatedType::VariableAnchorOffsetCollection;
    }
    if name.ends_with("-translate-anchor") {
        return EvaluatedType::Enum("TranslateAnchorType".into());
    }
    if ["-rotation-alignment", "-pitch-alignment", "-illumination-alignment"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
    {
        return EvaluatedType::Enum("AlignmentType".into());
    }
    if name == "text-anchor" || name == "icon-anchor" {
        return EvaluatedType::Enum("SymbolAnchorType".into());
    }
    if name.contains("position") {
        return EvaluatedType::Position;
    }
    match q.type_ {
        "boolean" => EvaluatedType::Bool,
        "number" => {
            // TODO: Check if 'Rotation' should be used for other properties,
            // such as icon-rotate
            // JS `==`: `property.period == 360 && property.units == 'degrees'`.
            let period_is_360 = match q.period {
                Some(Value::Number(n)) => n.as_f64() == Some(360.0),
                Some(Value::String(s)) => s.trim().parse::<f64>().ok() == Some(360.0),
                _ => false,
            };
            if name.ends_with("bearing") && period_is_360 && q.units == Some("degrees") {
                return EvaluatedType::Rotation;
            }
            if name.ends_with("location") { EvaluatedType::Double } else { EvaluatedType::Float }
        }
        "numberArray" => EvaluatedType::Vector(Box::new(EvaluatedType::Float)),
        "colorArray" => EvaluatedType::Vector(Box::new(EvaluatedType::Color)),
        "resolvedImage" => EvaluatedType::Image,
        "formatted" => EvaluatedType::Formatted,
        "string" => EvaluatedType::String,
        "enum" => EvaluatedType::Enum(format!("{}{}Type", if q.light { "Light" } else { "" }, camelize(name))),
        "color" => EvaluatedType::Color,
        "padding" => EvaluatedType::Padding,
        "array" => {
            let element = evaluated_type_of(&TypeQuery {
                name: q.name,
                type_: q.value.unwrap_or_else(|| panic!("{name}: array without a value type")),
                value: None,
                length: None,
                period: None,
                units: None,
                light: false,
            });
            match q.length {
                Some(n) => EvaluatedType::Array(Box::new(element), n),
                None => EvaluatedType::Vector(Box::new(element)),
            }
        }
        other => panic!("unknown type {other} for {name}"),
    }
}

#[must_use]
pub(crate) fn evaluated_type(property: &Property) -> EvaluatedType {
    evaluated_type_of(&TypeQuery {
        name: Some(&property.name),
        type_: property.type_(),
        value: property.value(),
        length: property.length(),
        period: property.period(),
        units: property.units(),
        light: property.light,
    })
}

/// The evaluated type of an array element of type `type_`, which has no name.
#[must_use]
pub(crate) fn evaluated_type_unnamed(type_: &str) -> EvaluatedType {
    evaluated_type_of(&TypeQuery {
        name: None,
        type_,
        value: None,
        length: None,
        period: None,
        units: None,
        light: false,
    })
}

/// The expression type of an overridable property's values, as a Rust `Type`
/// expression.
#[must_use]
pub(crate) fn expression_type_rust(type_: &str, value: Option<&str>) -> String {
    match type_ {
        "boolean" => "Type::Boolean".into(),
        "number" | "enum" => "Type::Number".into(),
        "numberArray" => "Type::array(Type::Number)".into(),
        "colorArray" => "Type::array(Type::Color)".into(),
        "image" => "Type::Image".into(),
        "string" => "Type::String".into(),
        "color" => "Type::Color".into(),
        "padding" => "Type::Padding".into(),
        "variableAnchorOffsetCollection" => "Type::VariableAnchorOffsetCollection".into(),
        "formatted" => "Type::Formatted".into(),
        "array" => format!("Type::array({})", expression_type_rust(value.expect("array value type"), None)),
        other => panic!("unknown type {other} for expressionType"),
    }
}

/// `property['property-type']` is data-driven for `convert<>` and the layout
/// property kind.
#[must_use]
pub(crate) fn is_data_driven(property: &Property) -> bool {
    matches!(property.property_type(), Some("data-driven" | "cross-faded-data-driven"))
}

#[must_use]
pub(crate) fn is_color_ramp(property: &Property) -> bool {
    property.property_type() == Some("color-ramp")
}

/// The Rust type of the property's style value.
#[must_use]
pub(crate) fn property_value_type(property: &Property) -> String {
    if is_color_ramp(property) {
        "ColorRampPropertyValue".into()
    } else {
        format!("PropertyValue<{}>", evaluated_type(property).rust())
    }
}
