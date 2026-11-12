// Ports the type-mapping helpers of scripts/generate-style-code.mjs and
// scripts/style-code.mjs: `camelize`, `camelizeWithLeadingLowercase`,
// `isOverridable`, `expressionType`, `evaluatedType`, `attributeUniformType`,
// `layoutPropertyType`, `paintPropertyType`, `propertyValueType`.

use serde_json::Value;

use crate::spec::Property;

/// `camelize`: `str.replace(/(?:^|-)(.)/g, (_, x) => x.toUpperCase())`.
#[must_use]
pub fn camelize(s: &str) -> String {
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

/// `camelizeWithLeadingLowercase`: `str.replace(/-(.)/g, (_, x) => x.toUpperCase())`.
#[must_use]
pub fn camelize_with_leading_lowercase(s: &str) -> String {
    let mut result = String::new();
    let mut upper_next = false;
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
pub fn snake(s: &str) -> String {
    s.replace('-', "_")
}

/// `isOverridable`.
#[must_use]
pub fn is_overridable(property: &Property) -> bool {
    ["text-color"].contains(&property.name.as_str())
}

/// The C++ `evaluatedType` of a property, as a type the generator can print
/// both as C++ (for comments) and as Rust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvaluatedType {
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
    /// `std::array<T, N>`.
    Array(Box<EvaluatedType>, usize),
    /// `std::vector<T>`.
    Vector(Box<EvaluatedType>),
}

impl EvaluatedType {
    #[must_use]
    pub fn cpp(&self) -> String {
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
    pub fn rust(&self) -> String {
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
    pub fn named_types(&self, out: &mut Vec<String>) {
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

/// `evaluatedType(property)`.
#[must_use]
pub fn evaluated_type(property: &Property) -> EvaluatedType {
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

/// `evaluatedType({type, name: undefined})`, used by `formatNumber` /
/// `defaultValue` for array elements.
#[must_use]
pub fn evaluated_type_unnamed(type_: &str) -> EvaluatedType {
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

/// `expressionType(property)` (the `expression::type` of an overridable
/// property), as the Rust `Type` expression.
#[must_use]
pub fn expression_type_rust(type_: &str, value: Option<&str>) -> String {
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

/// `attributeUniformType(property, type)`.
pub fn attribute_uniform_type(property: &Property, layer_type: &str) -> String {
    let exceptions: &[(&str, &[&str])] = &[
        ("text-opacity", &["opacity"]),
        ("icon-opacity", &["opacity"]),
        ("text-color", &["fill_color"]),
        ("icon-color", &["fill_color"]),
        ("text-halo-color", &["halo_color"]),
        ("icon-halo-color", &["halo_color"]),
        ("text-halo-blur", &["halo_blur"]),
        ("icon-halo-blur", &["halo_blur"]),
        ("text-halo-width", &["halo_width"]),
        ("icon-halo-width", &["halo_width"]),
        ("line-gap-width", &["gapwidth"]),
        ("line-pattern", &["pattern_to", "pattern_from"]),
        ("line-floor-width", &["floorwidth"]),
        ("fill-pattern", &["pattern_to", "pattern_from"]),
        ("fill-extrusion-pattern", &["pattern_to", "pattern_from"]),
    ];
    let names: Vec<String> = match exceptions.iter().find(|(n, _)| *n == property.name) {
        Some((_, names)) => names.iter().map(std::string::ToString::to_string).collect(),
        // `property.name.replace(type + '-', '').replace(/-/g, '_')`: the
        // first occurrence only.
        None => vec![property.name.replacen(&format!("{layer_type}-"), "", 1).replace('-', "_")],
    };
    names.iter().map(|name| format!("attributes::{name}, uniforms::{name}")).collect::<Vec<_>>().join(", ")
}

/// `property['property-type']` is data-driven for `convert<>` and the layout
/// property kind.
#[must_use]
pub fn is_data_driven(property: &Property) -> bool {
    matches!(property.property_type(), Some("data-driven" | "cross-faded-data-driven"))
}

#[must_use]
pub fn is_color_ramp(property: &Property) -> bool {
    property.property_type() == Some("color-ramp")
}

/// `layoutPropertyType(property)` (C++ spelling, for comments).
#[must_use]
pub fn layout_property_type_cpp(property: &Property) -> String {
    if is_data_driven(property) {
        format!("DataDrivenLayoutProperty<{}>", evaluated_type(property).cpp())
    } else {
        format!("LayoutProperty<{}>", evaluated_type(property).cpp())
    }
}

/// `paintPropertyType(property, type)` (C++ spelling, for comments). The
/// color-ramp case is the template's `ColorRampProperty`.
#[must_use]
pub fn paint_property_type_cpp(property: &Property, layer_type: &str) -> String {
    let t = || evaluated_type(property).cpp();
    match property.property_type() {
        Some("color-ramp") => "ColorRampProperty".into(),
        Some("data-driven") => {
            if is_overridable(property) {
                format!("DataDrivenPaintProperty<{}, {}, true>", t(), attribute_uniform_type(property, layer_type))
            } else {
                format!("DataDrivenPaintProperty<{}, {}>", t(), attribute_uniform_type(property, layer_type))
            }
        }
        Some("cross-faded-data-driven") => {
            format!("CrossFadedDataDrivenPaintProperty<{}, {}>", t(), attribute_uniform_type(property, layer_type))
        }
        Some("cross-faded") => format!("CrossFadedPaintProperty<{}>", t()),
        _ => format!("PaintProperty<{}>", t()),
    }
}

/// `propertyValueType(property)` in Rust.
#[must_use]
pub fn property_value_type(property: &Property) -> String {
    if is_color_ramp(property) {
        "ColorRampPropertyValue".into()
    } else {
        format!("PropertyValue<{}>", evaluated_type(property).rust())
    }
}
