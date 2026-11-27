//! The crate's style members, the values they parse to, and the router that
//! turns them into the frames' shared targets and the layers' routes.

use std::sync::Arc;

use mln_engine::layer::{Router, Routes};
use mln_gfx::types::TextureChannelDataType;
use mln_style::Style;
use mln_style::color::Color;
use mln_style::conversion::{Converter, Convertible, Error};
use mln_style::expression::{Functions, ValueConverter};
use mln_style::member::Member;
use mln_style::property::convert::convert_property_value;
use mln_style::property::value::PropertyValue;

/// The style's root `org.maplibre:targets`.
pub(crate) static TARGETS: Member<Targets> = Member::root("org.maplibre:targets", parse_targets);

/// A layer's `org.maplibre:destination-target`, on a layer of any type.
pub(crate) static DESTINATION: Member<TargetName> =
    Member::layer("org.maplibre:destination-target", |value, _| name(value).map(TargetName));

/// An image-process layer's `org.maplibre:source-target`.
pub(crate) static SOURCE: Member<TargetName> =
    Member::layer("org.maplibre:source-target", |value, _| name(value).map(TargetName));

/// An image-process layer's `org.maplibre:operations`.
pub(crate) static OPERATIONS: Member<Operations> = Member::layer("org.maplibre:operations", parse_operations);

/// The shared targets the style declares, in its order, each with the type
/// of its texture's channels.
#[derive(Debug, PartialEq)]
pub(crate) struct Targets(pub(crate) Vec<(Arc<str>, TextureChannelDataType)>);

/// The name of a shared target that a layer draws into or reads.
#[derive(Debug, PartialEq)]
pub(crate) struct TargetName(pub(crate) Arc<str>);

/// What an image-process layer does to what it reads, in order.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Operations(pub(crate) Vec<Operation>);

/// One of an image-process layer's operations, on premultiplied colors,
/// whose alpha is their coverage. Its numbers and colors may follow the
/// zoom.
#[derive(Debug, PartialEq)]
pub(crate) enum Operation {
    /// A Gaussian blur whose standard deviation is `size` logical pixels,
    /// from 0 to 64.
    Blur { size: PropertyValue<f32> },
    /// Coverage above `cutoff`, from 0 to 255, made whole and below it
    /// none, antialiased over an edge centered on the cutoff.
    Threshold { cutoff: PropertyValue<f32> },
    /// Every pixel painted `value`, its coverage kept.
    Color { value: PropertyValue<Color> },
    /// Coverage scaled by `value`, from 0 to 1.
    Opacity { value: PropertyValue<f32> },
    /// The image put together with the shared target `target`'s, as `mode`
    /// says.
    Combine { target: Arc<str>, mode: Combine },
}

/// How [`Operation::Combine`] puts another target's image with the one it
/// processes. The shaders know each by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Combine {
    /// Only where the other covers: a mask.
    Multiply,
    /// Only where the other doesn't: a cut-out.
    Subtract,
    /// Each channel's more of the two.
    Max,
    /// Each channel's less of the two.
    Min,
    /// The other drawn over it.
    Over,
    /// The other drawn beneath it.
    Under,
}

const COMBINES: [(&str, Combine); 6] = [
    ("multiply", Combine::Multiply),
    ("subtract", Combine::Subtract),
    ("max", Combine::Max),
    ("min", Combine::Min),
    ("over", Combine::Over),
    ("under", Combine::Under),
];

/// The router of the crate's members: the style's targets, and each layer
/// with a destination routed into it.
pub(crate) struct TargetRouter;

impl Router for TargetRouter {
    fn route(&self, style: &Style, routes: &mut Routes) {
        for (name, type_) in style.members().get(&TARGETS).map_or(&[][..], |targets| &targets.0) {
            routes.target(name, *type_);
        }
        for layer in style.get_layers() {
            if let Some(TargetName(to)) = layer.members().get(&DESTINATION) {
                routes.route(layer.get_id(), to);
            }
        }
    }
}

fn name(value: &Convertible<'_>) -> Result<Arc<str>, Error> {
    value.to_string().map(Arc::from).ok_or_else(|| Error::new("expected a target's id"))
}

/// Fails on a member of the object `value` that isn't among `members`.
fn only(value: &Convertible<'_>, members: &[&str]) -> Result<(), Error> {
    let unknown = |name: &str, _: &Convertible<'_>| {
        (!members.contains(&name)).then(|| Error::new(format!("no member \"{name}\"")))
    };
    value.each_member(unknown).map_or(Ok(()), Err)
}

/// Fails on a value that isn't an array, and on the first of its elements
/// that `element` fails on, with the element's index.
fn array<T>(
    value: &Convertible<'_>,
    of: &str,
    mut element: impl FnMut(&Convertible<'_>) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    if !value.is_array() {
        return Err(Error::new(format!("expected an array of {of}")));
    }
    let at = |i: usize| move |error: Error| Error::new(format!("[{i}]: {}", error.message));
    (0..value.array_length()).map(|i| element(&value.array_member(i)).map_err(at(i))).collect()
}

fn parse_targets(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<Targets, Error> {
    let mut names: Vec<Arc<str>> = Vec::new();
    let targets = array(value, "targets", |target| {
        if !target.is_object() {
            return Err(Error::new("expected a target"));
        }
        only(target, &["id", "precision"])?;
        let name = target.object_member("id").ok_or_else(|| Error::new("expected an id"))?;
        let name = self::name(&name)?;
        if names.contains(&name) {
            return Err(Error::new(format!("a second target \"{name}\"")));
        }
        names.push(Arc::clone(&name));
        let type_ = match target.object_member("precision").map(|precision| precision.to_string()) {
            None => TextureChannelDataType::UnsignedByte,
            Some(Some(precision)) if precision == "standard" => TextureChannelDataType::UnsignedByte,
            Some(Some(precision)) if precision == "high" => TextureChannelDataType::HalfFloat,
            Some(_) => return Err(Error::new("expected a precision of \"standard\" or \"high\"")),
        };
        Ok((name, type_))
    })?;
    Ok(Targets(targets))
}

fn parse_operations(value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Operations, Error> {
    array(value, "operations", |operation| parse_operation(operation, functions)).map(Operations)
}

/// The value of the member `name` of `object`, which may follow the zoom
/// but not the feature, or `None` without one.
fn param<T: Converter + ValueConverter + Clone>(
    object: &Convertible<'_>,
    name: &str,
    functions: &Arc<Functions>,
) -> Result<Option<PropertyValue<T>>, Error> {
    let Some(member) = object.object_member(name) else { return Ok(None) };
    let mut error = Error::default();
    match convert_property_value(&member, &mut error, false, false, functions) {
        Some(PropertyValue::Undefined) => Ok(None),
        Some(value) => Ok(Some(value)),
        None => Err(Error::new(format!("{name}: {}", error.message))),
    }
}

/// The value of the member `name` of `object`, which it must have.
fn required<T: Converter + ValueConverter + Clone>(
    object: &Convertible<'_>,
    name: &str,
    functions: &Arc<Functions>,
) -> Result<PropertyValue<T>, Error> {
    param(object, name, functions)?.ok_or_else(|| Error::new(format!("expected a {name}")))
}

fn parse_operation(value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Operation, Error> {
    let kind = value.object_member("type").and_then(|kind| kind.to_string());
    let kind = kind.ok_or_else(|| Error::new("expected an operation's type"))?;
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
            let target = value.object_member("target").ok_or_else(|| Error::new("expected a target"))?;
            let target = name(&target)?;
            let mode = match value.object_member("mode") {
                None => Combine::Multiply,
                Some(mode) => {
                    let mode = mode.to_string();
                    let known = COMBINES.iter().find(|(name, _)| Some(*name) == mode.as_deref());
                    known
                        .map(|&(_, mode)| mode)
                        .ok_or_else(|| Error::new("expected a mode of multiply, subtract, max, min, over or under"))?
                }
            };
            Operation::Combine { target, mode }
        }
        other => {
            return Err(Error::new(format!(
                "no operation \"{other}\": expected blur, threshold, color, opacity or combine"
            )));
        }
    };
    Ok(operation)
}

#[cfg(test)]
mod tests {
    use mln_engine::RegistryBuilder;
    use mln_gfx::Size;
    use mln_style::error::ErrorKind;

    use super::*;
    use crate::plan::plan;

    /// A style of `layers` with the built-ins and the crate's members.
    fn style(layers: &str) -> Style {
        let mut builder = RegistryBuilder::new();
        mln_builtins::register_builtins(&mut builder).unwrap();
        crate::register(&mut builder).unwrap();
        let json = format!(r#"{{"version":8,"sources":{{}},"layers":[{layers}]}}"#);
        Style::from_json(builder.build().style(), &json).unwrap()
    }

    const PROCESS: &str = r#"{"id":"process","type":"org.maplibre.image-process"}"#;

    /// The message of the error of setting the root member `name` to `json`.
    #[track_caller]
    fn root_refusal(name: &str, json: &str) -> String {
        let error = style("").set_root_property_json(name, json).unwrap_err();
        assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
        error.message
    }

    /// The message of the error of setting an image-process layer's member
    /// `name` to `json`.
    #[track_caller]
    fn layer_refusal(name: &str, json: &str) -> String {
        let error = style(PROCESS).set_layer_property_json("process", name, json).unwrap_err();
        assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
        error.message
    }

    #[track_caller]
    fn assert_refused(message: &str, says: &str) {
        assert!(message.contains(says), "{message:?} doesn't say {says:?}");
    }

    #[test]
    fn the_targets_refuse_what_isnt_a_list_of_distinct_targets() {
        let targets = "org.maplibre:targets";
        assert_refused(&root_refusal(targets, r#"{"id":"a"}"#), "expected an array of targets");
        assert_refused(&root_refusal(targets, r#"["a"]"#), "[0]: expected a target");
        assert_refused(&root_refusal(targets, r#"[{"precision":"high"}]"#), "[0]: expected an id");
        assert_refused(&root_refusal(targets, r#"[{"id":1}]"#), "[0]: expected a target's id");
        assert_refused(&root_refusal(targets, r#"[{"id":"a","size":2}]"#), r#"[0]: no member "size""#);
        assert_refused(&root_refusal(targets, r#"[{"id":"a"},{"id":"a"}]"#), r#"[1]: a second target "a""#);
        let precision = root_refusal(targets, r#"[{"id":"a","precision":"low"}]"#);
        assert_refused(&precision, r#"[0]: expected a precision of "standard" or "high""#);
    }

    #[test]
    fn the_target_names_refuse_what_isnt_a_name() {
        assert_refused(&layer_refusal("org.maplibre:destination-target", "1"), "expected a target's id");
        assert_refused(&layer_refusal("org.maplibre:source-target", r#"["a"]"#), "expected a target's id");
    }

    #[test]
    fn the_operations_refuse_unknown_operations_members_and_values() {
        let operations = "org.maplibre:operations";
        assert_refused(&layer_refusal(operations, r#"{"type":"blur"}"#), "expected an array of operations");
        assert_refused(&layer_refusal(operations, r#"[{"size":2}]"#), "[0]: expected an operation's type");
        assert_refused(&layer_refusal(operations, r#"[{"type":"sharpen"}]"#), r#"[0]: no operation "sharpen""#);
        let unknown = layer_refusal(operations, r#"[{"type":"color","value":"red"},{"type":"blur","size":2,"x":1}]"#);
        assert_refused(&unknown, r#"[1]: no member "x""#);
        assert_refused(&layer_refusal(operations, r#"[{"type":"blur"}]"#), "[0]: expected a size");
        assert_refused(&layer_refusal(operations, r#"[{"type":"opacity","value":"half"}]"#), "[0]: value: ");
        let data = layer_refusal(operations, r#"[{"type":"blur","size":["get","size"]}]"#);
        assert_refused(&data, "[0]: size: data expressions not supported");
        assert_refused(&layer_refusal(operations, r#"[{"type":"combine"}]"#), "[0]: expected a target");
        let mode = layer_refusal(operations, r#"[{"type":"combine","target":"a","mode":"screen"}]"#);
        assert_refused(&mode, "[0]: expected a mode of multiply, subtract, max, min, over or under");
    }

    #[test]
    fn a_style_writes_back_the_members_it_parsed() {
        let targets = r#"[{"id":"shapes"},{"id":"soft","precision":"high"}]"#;
        // In the order the style writes members out: by name.
        let operations = r##"[{"size":{"stops":[[0,1],[10,4]]},"type":"blur"},{"type":"threshold"},{"type":"color","value":"#108880"},{"type":"opacity","value":0.5},{"mode":"over","target":"shapes","type":"combine"}]"##;
        let process = format!(
            r#"{{"id":"process","org.maplibre:destination-target":"soft","org.maplibre:operations":{operations},"org.maplibre:source-target":"shapes","type":"org.maplibre.image-process"}}"#
        );
        let mut style = style(&process);
        style.set_root_property_json("org.maplibre:targets", targets).unwrap();
        assert_eq!(Some(targets.to_owned()), style.root_property_json("org.maplibre:targets").unwrap());
        let layer = style.layer_json("process").unwrap();
        assert_eq!(process, layer);

        let members = style.members();
        let declared = [
            ("shapes".into(), TextureChannelDataType::UnsignedByte),
            ("soft".into(), TextureChannelDataType::HalfFloat),
        ];
        assert_eq!(Some(&Targets(declared.into())), members.get(&TARGETS));
        // A layer's JSON adds back as the same layer.
        let mut copy = self::style("");
        copy.add_layer_json(&layer, None).unwrap();
        let (layer, copied) = (style.get_layer("process").unwrap(), copy.get_layer("process").unwrap());
        assert_eq!(layer.members(), copied.members());
        let parsed = copied.members().get(&OPERATIONS).unwrap();
        assert_eq!(layer.members().get(&OPERATIONS), Some(parsed));
        assert_eq!(
            Some(&Combine::Over),
            parsed.0.iter().find_map(|op| match op {
                Operation::Combine { mode, .. } => Some(mode),
                _ => None,
            })
        );
    }

    #[test]
    fn an_operations_values_follow_the_zoom() {
        let json = r#"[{"type":"threshold","cutoff":["interpolate",["linear"],["zoom"],0,0,10,255]}]"#;
        let mut style = style(PROCESS);
        style.set_layer_property_json("process", "org.maplibre:operations", json).unwrap();
        let operations = style.get_layer("process").unwrap().members().get(&OPERATIONS).unwrap();
        let size = Size::new(4, 4);
        let cutoff = |zoom| plan(&operations.0, zoom, size, size)[0].block.ops[0][1];
        assert_eq!([cutoff(0.0), cutoff(5.0), cutoff(10.0)], [0.0, 0.5, 1.0]);
    }
}
