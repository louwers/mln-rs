//! The style keys of what an `org.maplibre.image-process` layer reads and
//! does.

use std::sync::Arc;

use mln_rs::feature::{Value, ValueObject};
use mln_rs::layer::{Color, Convertible, Functions, PropertyValue, StyleKey, conversion};
use mln_rs::mln_style::StyleValue;
use mln_rs::mln_style::conversion::Converter;
use mln_rs::mln_style::expression::ValueConverter;

use crate::ImageProcess;

/// An `org.maplibre.image-process` layer's `org.maplibre:source-target`.
pub static SOURCE: StyleKey<SourceTarget> =
    StyleKey::layer_of::<ImageProcess>("org.maplibre:source-target", parse_source, |source| {
        Value::String(source.0.to_string())
    });

/// An `org.maplibre.image-process` layer's `org.maplibre:operations`.
pub static OPERATIONS: StyleKey<Operations> =
    StyleKey::layer_of::<ImageProcess>("org.maplibre:operations", parse_operations, operations_json);

/// The named target an image-process layer reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceTarget(pub Arc<str>);

/// What an image-process layer does to what it reads, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Operations(pub Vec<Operation>);

/// One of an image-process layer's operations, on premultiplied colors,
/// whose alpha is their coverage. Numbers and colors may follow the zoom.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// A Gaussian blur whose standard deviation is `size` logical pixels,
    /// from 0 to 64.
    Blur { size: PropertyValue<f32> },
    /// Coverage at or above `cutoff`, from 0 to 255, made whole and the
    /// rest none, with an antialiased edge.
    Threshold { cutoff: PropertyValue<f32> },
    /// Every pixel painted `value`, its coverage kept.
    Color { value: PropertyValue<Color> },
    /// Coverage scaled by `value`, from 0 to 1.
    Opacity { value: PropertyValue<f32> },
    /// The image put together with the named target `target`'s, as `mode`
    /// says.
    Combine { target: Arc<str>, mode: Combine },
}

/// How [`Operation::Combine`] puts another target's image with the one
/// processed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// Only where the other covers: a mask.
    Multiply,
    /// Only where the other doesn't: a cut-out.
    Subtract,
    /// The more covered of the two: a union.
    Max,
    /// The less covered of the two: an intersection.
    Min,
    /// The other drawn over it.
    Over,
    /// The other drawn beneath it.
    Under,
}

impl Combine {
    const NAMES: [(&str, Combine); 6] = [
        ("multiply", Combine::Multiply),
        ("subtract", Combine::Subtract),
        ("max", Combine::Max),
        ("min", Combine::Min),
        ("over", Combine::Over),
        ("under", Combine::Under),
    ];

    fn name(self) -> &'static str {
        Combine::NAMES.iter().find(|(_, mode)| *mode == self).map_or("", |(name, _)| name)
    }
}

fn error(message: impl Into<String>) -> conversion::Error {
    conversion::Error::new(message)
}

fn string(value: &Convertible<'_>, what: &str) -> Result<Arc<str>, conversion::Error> {
    value.to_string().map(Into::into).ok_or_else(|| error(format!("expected {what}")))
}

/// Fails on a member of `object` not among `members`.
fn only(object: &Convertible<'_>, members: &[&str]) -> Result<(), conversion::Error> {
    object
        .each_member(|name, _| (!members.contains(&name)).then(|| error(format!("no member \"{name}\""))))
        .map_or(Ok(()), Err)
}

fn parse_source(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<SourceTarget, conversion::Error> {
    string(value, "a target's id").map(SourceTarget)
}

fn parse_operations(value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Operations, conversion::Error> {
    if !value.is_array() {
        return Err(error("expected an array of operations"));
    }
    let operation =
        |i| parse_operation(&value.array_member(i), functions).map_err(|e| error(format!("[{i}]: {}", e.message)));
    (0..value.array_length()).map(operation).collect::<Result<_, _>>().map(Operations)
}

/// The value of `object`'s member `name`, which may follow the zoom, or
/// `None` without one.
fn param<T: Converter + ValueConverter + Clone>(
    object: &Convertible<'_>,
    name: &str,
    functions: &Arc<Functions>,
) -> Result<Option<PropertyValue<T>>, conversion::Error> {
    let Some(member) = object.object_member(name) else { return Ok(None) };
    PropertyValue::parse(&member, functions).map(Some).map_err(|e| error(format!("{name}: {}", e.message)))
}

/// The value of `object`'s member `name`, which it must have.
fn required<T: Converter + ValueConverter + Clone>(
    object: &Convertible<'_>,
    name: &str,
    functions: &Arc<Functions>,
) -> Result<PropertyValue<T>, conversion::Error> {
    param(object, name, functions)?.ok_or_else(|| error(format!("expected a {name}")))
}

fn parse_operation(value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Operation, conversion::Error> {
    let kind = value.object_member("type").and_then(|kind| kind.to_string());
    let kind = kind.ok_or_else(|| error("expected an operation's type"))?;
    let operation = match kind.as_str() {
        "blur" => {
            only(value, &["type", "size"])?;
            Operation::Blur { size: required(value, "size", functions)? }
        }
        "threshold" => {
            only(value, &["type", "cutoff"])?;
            Operation::Threshold {
                cutoff: param(value, "cutoff", functions)?.unwrap_or(PropertyValue::Constant(128.0)),
            }
        }
        "color" => {
            only(value, &["type", "value"])?;
            Operation::Color { value: required(value, "value", functions)? }
        }
        "opacity" => {
            only(value, &["type", "value"])?;
            Operation::Opacity { value: required(value, "value", functions)? }
        }
        "combine" => {
            only(value, &["type", "target", "mode"])?;
            let target = value.object_member("target").ok_or_else(|| error("expected a target"))?;
            let target = string(&target, "a target's id")?;
            let mode = match value.object_member("mode").map(|mode| mode.to_string()) {
                None => Combine::Multiply,
                Some(name) => Combine::NAMES
                    .iter()
                    .find(|(known, _)| Some(*known) == name.as_deref())
                    .map(|(_, mode)| *mode)
                    .ok_or_else(|| error("expected a mode of multiply, subtract, max, min, over or under"))?,
            };
            Operation::Combine { target, mode }
        }
        other => {
            return Err(error(format!(
                "no operation \"{other}\": expected blur, threshold, color, opacity or combine"
            )));
        }
    };
    Ok(operation)
}

/// The JSON of a value that may follow the zoom.
fn json(value: &impl StyleValue) -> Value {
    value.style_property().get_value().clone()
}

fn operations_json(operations: &Operations) -> Value {
    let operation = |operation: &Operation| {
        let mut json = ValueObject::new();
        let (kind, members): (&str, Vec<(&str, Value)>) = match operation {
            Operation::Blur { size } => ("blur", vec![("size", self::json(size))]),
            Operation::Threshold { cutoff } => ("threshold", vec![("cutoff", self::json(cutoff))]),
            Operation::Color { value } => ("color", vec![("value", self::json(value))]),
            Operation::Opacity { value } => ("opacity", vec![("value", self::json(value))]),
            Operation::Combine { target, mode } => (
                "combine",
                vec![("target", Value::String(target.to_string())), ("mode", Value::String(mode.name().to_owned()))],
            ),
        };
        json.insert("type".to_owned(), Value::String(kind.to_owned()));
        json.extend(members.into_iter().map(|(name, value)| (name.to_owned(), value)));
        Value::Object(json)
    };
    Value::Array(operations.0.iter().map(operation).collect())
}
