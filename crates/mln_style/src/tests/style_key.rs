//! Style keys: members of the root and of layers that keys declared here
//! give, read by type, edited, diffed and serialized.

use std::sync::Arc;

use feature::Value;
use mln_util::{Error as StyleError, ErrorKind};

use super::util::FixtureLog;
use crate::conversion::{Convertible, Error, convert_property_value};
use crate::diff::{LayerChange, StyleDiff};
use crate::expression::Functions;
use crate::expression::functions::RegisterError;
use crate::layer_type::{LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::property_table::{PropertyInfo, PropertyTable};
use crate::property_value::{PropertyValue, StyleValue as _};
use crate::style_key::{StyleKey, StyleKeys};
use crate::tests::fixture_layer::Plain;
use crate::{Style, StyleRegistry};

#[derive(Debug, PartialEq)]
struct Tag(String);

#[derive(Debug, PartialEq)]
struct Route(String);

/// A value that may follow the zoom, which its reader evaluates.
#[derive(Debug, PartialEq)]
struct Level(PropertyValue<f32>);

static TAG: StyleKey<Tag> = StyleKey::root("test:tag", |value, _| string(value).map(Tag), |tag| string_json(&tag.0));
static ROUTE: StyleKey<Route> =
    StyleKey::layer("test:route", |value, _| string(value).map(Route), |route| string_json(&route.0));
static LEVEL: StyleKey<Level> =
    StyleKey::layer_of::<Plain>("test:level", parse_level, |level| level.0.style_property().get_value().clone());

fn string(value: &Convertible<'_>) -> Result<String, Error> {
    value.to_string().ok_or_else(|| Error::new("expected a string"))
}

fn string_json(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn parse_level(value: &Convertible<'_>, functions: &Arc<Functions>) -> Result<Level, Error> {
    let mut error = Error::default();
    convert_property_value(value, &mut error, false, false, functions).map(Level).ok_or(error)
}

fn registry() -> Arc<StyleRegistry> {
    let mut keys = StyleKeys::default();
    keys.add(&TAG).unwrap();
    keys.add(&ROUTE).unwrap();
    keys.add(&LEVEL).unwrap();
    StyleRegistry::with_keys(keys)
}

const STYLE: &str = r#"{"version":8,"test:tag":"night",
    "sources":{"points":{"type":"points"}},
    "layers":[
        {"id":"bg","type":"plain","test:route":"glow","test:level":["interpolate",["linear"],["zoom"],0,1,10,2]},
        {"id":"dots","type":"dot","source":"points","test:route":"blur"}]}"#;

fn style() -> Style {
    style_of(STYLE)
}

fn style_of(json: &str) -> Style {
    Style::from_json(&registry(), json, None).unwrap()
}

fn extra<'a, T: 'static>(style: &'a Style, layer: &str) -> Option<&'a T> {
    style.get_layer(layer).unwrap().style().base().extra()
}

fn warnings(json: &str) -> Vec<String> {
    let log = FixtureLog::new();
    style_of(json);
    log.messages().into_iter().map(|message| message.msg).collect()
}

#[track_caller]
fn assert_fails(result: Result<impl std::fmt::Debug, StyleError>, message: &str) {
    let error = result.expect_err("the call should fail");
    assert_eq!(error.kind, ErrorKind::InvalidArgument, "{}", error.message);
    assert!(error.message.contains(message), "{:?} doesn't contain {message:?}", error.message);
}

#[test]
fn the_style_reads_a_key_of_the_root_by_its_type() {
    let log = FixtureLog::new();
    let style = style();
    assert!(log.is_empty(), "{:?}", log.messages());
    assert_eq!(Some(&Tag("night".to_owned())), style.extra::<Tag>());
    assert_eq!(None, style.extra::<Route>());
    assert_eq!(Some(r#""night""#.to_owned()), style.root_property_json("test:tag").unwrap());
}

#[test]
fn a_key_of_layers_is_on_layers_of_any_type() {
    let style = style();
    assert_eq!(Some(&Route("glow".to_owned())), extra(&style, "bg"));
    assert_eq!(Some(&Route("blur".to_owned())), extra(&style, "dots"));
    assert_eq!(Some(r#""blur""#.to_owned()), style.layer_property_json("dots", "test:route").unwrap());
    assert_eq!(None, extra::<Tag>(&style, "bg"));
}

#[test]
fn a_key_of_one_layer_type_is_on_its_layers_alone() {
    let style = style();
    let level = extra::<Level>(&style, "bg").unwrap();
    assert!(level.0.is_expression(), "a key's value may follow the zoom");
    assert_eq!(None, extra::<Level>(&style, "dots"));

    let on_dots = r#"{"version":8,"sources":{"points":{"type":"points"}},
        "layers":[{"id":"dots","type":"dot","source":"points","test:level":1}]}"#;
    assert_eq!([r#"layer 'dots': the style key "test:level" belongs on "plain" layers"#], *warnings(on_dots));
    assert_eq!(None, extra::<Level>(&style_of(on_dots), "dots"));

    let mut style = style;
    let dots = r#"{"id":"more","type":"dot","source":"points","test:level":1}"#;
    assert_fails(style.add_layer_json(dots, None), r#""test:level" belongs on "plain" layers"#);
    assert_fails(style.set_layer_property_json("dots", "test:level", "1"), r#"belongs on "plain" layers"#);
}

#[test]
fn a_key_of_another_place_warns_on_load_and_fails_edits() {
    let misplaced = r#"{"version":8,"test:route":"glow","layers":[{"id":"bg","type":"plain","test:tag":"night"}]}"#;
    assert_eq!(
        [
            r#"layer 'bg': the style key "test:tag" belongs at the style's root"#,
            r#"the style key "test:route" belongs on layers"#,
        ],
        *warnings(misplaced)
    );
    let mut style = style_of(misplaced);
    assert_eq!((None, None), (style.extra::<Route>(), extra::<Tag>(&style, "bg")));

    assert_fails(style.set_layer_property_json("bg", "test:tag", r#""day""#), "belongs at the style's root");
    assert_fails(style.set_root_property_json("test:route", r#""glow""#), "belongs on layers");
    assert_fails(style.root_property_json("test:route"), "belongs on layers");
}

#[test]
fn an_unregistered_namespaced_member_warns_on_load_and_fails_edits() {
    let typo = r#"{"version":8,"test:tags":"night","layers":[{"id":"bg","type":"plain","test:rout":"glow"}]}"#;
    assert_eq!(
        [
            r#"layer 'bg': no extension registered the style key "test:rout""#,
            r#"no extension registered the style key "test:tags""#,
        ],
        *warnings(typo)
    );

    let mut style = style_of(typo);
    let layer = r#"{"id":"more","type":"plain","test:rout":"glow"}"#;
    assert_fails(style.add_layer_json(layer, None), r#"no extension registered the style key "test:rout""#);
    assert_fails(style.set_layer_property_json("bg", "test:rout", r#""glow""#), "test:rout");
    assert_fails(style.set_root_property_json("test:tags", r#""night""#), "no extension registered");
    assert_fails(style.root_property_json("test:tags"), "no extension registered");
}

#[test]
fn a_member_without_a_namespace_is_no_key() {
    let plain = r#"{"version":8,"tag":"night","layers":[{"id":"bg","type":"plain","route":"glow"}]}"#;
    assert_eq!(Vec::<String>::new(), warnings(plain));
    let mut style = style_of(plain);
    style.add_layer_json(r#"{"id":"more","type":"plain","route":"glow"}"#, None).unwrap();
    assert_fails(style.set_root_property_json("tag", r#""night""#), "is not a style root property");
}

#[test]
fn a_value_that_doesnt_parse_warns_and_leaves_the_key_unset() {
    let wrong = r#"{"version":8,"test:tag":1,"layers":[{"id":"bg","type":"plain","test:route":2}]}"#;
    assert_eq!(
        [r#"layer 'bg': style key "test:route": expected a string"#, r#"style key "test:tag": expected a string"#],
        *warnings(wrong)
    );
    let mut style = style_of(wrong);
    assert_eq!((None, None), (style.extra::<Tag>(), extra::<Route>(&style, "bg")));

    let layer = r#"{"id":"more","type":"plain","test:route":2}"#;
    assert_fails(style.add_layer_json(layer, None), r#"style key "test:route": expected a string"#);
    assert_fails(style.set_layer_property_json("bg", "test:route", "2"), "expected a string");
    assert_fails(style.set_root_property_json("test:tag", "1"), "expected a string");
}

#[test]
#[should_panic(expected = "a style key's name is `<namespace>:<key>`")]
fn a_key_without_a_namespace_isnt_one() {
    let _ = StyleKey::<Tag>::root("tag", |value, _| string(value).map(Tag), |tag| string_json(&tag.0));
}

#[test]
#[should_panic(expected = "a style key's name is `<namespace>:<key>`")]
fn a_key_with_an_empty_namespace_isnt_one() {
    let _ = StyleKey::<Tag>::layer(":tag", |value, _| string(value).map(Tag), |tag| string_json(&tag.0));
}

#[test]
fn edits_set_and_unset_keys() {
    let base = style();

    let mut style = base.clone();
    style.set_root_property_json("test:tag", r#""day""#).unwrap();
    assert_eq!(Some(&Tag("day".to_owned())), style.extra::<Tag>());
    assert!(!style.is_same_version(&base));
    let day = style.clone();
    style.set_root_property_json("test:tag", r#""day""#).unwrap();
    assert!(style.is_same_version(&day), "an edit that changes nothing keeps the version");
    style.set_root_property_json("test:tag", "null").unwrap();
    assert_eq!((None, None), (style.extra::<Tag>(), style.root_property_json("test:tag").unwrap()));

    let mut style = base.clone();
    style.set_layer_property_json("dots", "test:route", r#""fade""#).unwrap();
    assert_eq!(Some(&Route("fade".to_owned())), extra(&style, "dots"));
    let fade = style.clone();
    style.set_layer_property_json("dots", "test:route", r#""fade""#).unwrap();
    assert!(style.is_same_version(&fade), "an edit that changes nothing keeps the version");
    style.set_layer_property_json("dots", "test:route", "null").unwrap();
    assert_eq!(None, extra::<Route>(&style, "dots"));
    assert_eq!(None, style.layer_property_json("dots", "test:route").unwrap());
}

#[test]
fn keys_round_trip_through_json() {
    let mut style = style();
    let bg = style.layer_json("bg").unwrap();
    let json: serde_json::Value = serde_json::from_str(&bg).unwrap();
    assert_eq!(json["test:route"], "glow");
    assert_eq!(json["test:level"], serde_json::json!(["interpolate", ["linear"], ["zoom"], 0.0, 1.0, 10.0, 2.0]));

    let before = style.get_layer("bg").unwrap().style().base().extras.clone();
    style.apply(crate::Edit::RemoveLayer("bg".to_owned())).unwrap();
    style.add_layer_json(&bg, None).unwrap();
    assert_eq!(before, style.get_layer("bg").unwrap().style().base().extras);

    let tag = style.root_property_json("test:tag").unwrap().unwrap();
    let mut reset = style.clone();
    reset.set_root_property_json("test:tag", "null").unwrap();
    reset.set_root_property_json("test:tag", &tag).unwrap();
    assert_eq!(style.extra::<Tag>(), reset.extra::<Tag>());
}

#[test]
fn a_change_to_keys_alone_is_a_data_change() {
    let base = style();
    let mut keyed = base.clone();
    keyed.set_layer_property_json("dots", "test:route", r#""fade""#).unwrap();
    let diff = StyleDiff::between(Some(&base), &keyed);
    assert_eq!(Some(LayerChange::Data), diff.layers.get("dots"));
    assert!(!diff.layers.order_changed);

    let mut restyled = keyed.clone();
    restyled.set_layer_property_json("dots", "dot-color", r#""red""#).unwrap();
    assert_eq!(Some(LayerChange::Restyled), StyleDiff::between(Some(&base), &restyled).layers.get("dots"));

    // A new style, as `setStyle` gives, of layers of a type without data.
    let two = |route: &str| {
        style_of(&format!(
            r#"{{"version":8,"sources":{{"points":{{"type":"points"}}}},"layers":[
                {{"id":"same","type":"dot","source":"points","test:route":"blur"}},
                {{"id":"keyed","type":"dot","source":"points","test:route":"{route}"}}]}}"#
        ))
    };
    let diff = StyleDiff::between(Some(&two("blur")), &two("fade"));
    assert_eq!(Some(LayerChange::Data), diff.layers.get("keyed"));
    assert_eq!(Some(LayerChange::Restyled), diff.layers.get("same"), "the same values, parsed again");

    let mut tagged = base.clone();
    tagged.set_root_property_json("test:tag", r#""day""#).unwrap();
    assert!(StyleDiff::between(Some(&base), &tagged).layers.is_empty());
    assert!(!tagged.is_same_version(&base));
}

#[test]
fn a_ref_layer_has_the_keys_of_the_layer_it_refers_to() {
    let style = style_of(
        r#"{"version":8,"layers":[
            {"id":"bg","type":"plain","test:route":"glow"},
            {"id":"copy","ref":"bg","test:route":"other"}]}"#,
    );
    assert_eq!(Some(&Route("glow".to_owned())), extra(&style, "copy"));
}

#[test]
fn keys_dont_cross_to_tile_workers() {
    let style = style();
    let dots = style.get_layer("dots").unwrap().style();
    let mut writer = mln_wire::Writer::new();
    dots.write(&mut writer);
    let bytes = writer.finish().unwrap();
    let registry = registry();
    let mut context: &StyleRegistry = &registry;
    let mut reader = mln_wire::Reader::new(&bytes).with_context(&mut context);
    let read = dots.layer_type().read_style(&mut reader).unwrap();
    assert_eq!("dots", read.base().id);
    assert_eq!(None, read.base().extra::<Route>());
}

/// A layer type with a property whose name has a `:`.
struct Odd;

impl StyleLayerType for Odd {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "odd", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PropertyTable::unsorted(&[PropertyInfo::paint("test:route")]);
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// A key of another value type than [`TAG`], of its name.
static SAME_NAME: StyleKey<Route> =
    StyleKey::layer("test:tag", |value, _| string(value).map(Route), |route| string_json(&route.0));
/// A key of [`TAG`]'s value type, of another name.
static SAME_TYPE: StyleKey<Tag> =
    StyleKey::root("test:label", |value, _| string(value).map(Tag), |tag| string_json(&tag.0));

#[test]
fn a_registry_has_one_key_per_name_and_value_type() {
    let mut keys = StyleKeys::default();
    keys.add(&TAG).unwrap();
    assert_eq!(Ok(()), keys.add(&TAG), "a key added again changes nothing");
    assert_eq!(Err(RegisterError::Duplicate("test:tag".to_owned())), keys.add(&SAME_NAME));
    assert!(matches!(keys.add(&SAME_TYPE), Err(RegisterError::Duplicate(name)) if name.contains("Tag")));
}

#[test]
fn a_key_and_a_property_cant_share_a_name_in_either_order() {
    let mut keys = StyleKeys::default();
    keys.add_layer_type::<Odd>().unwrap();
    assert_eq!(Err(RegisterError::Duplicate("test:route".to_owned())), keys.add(&ROUTE));

    let mut keys = StyleKeys::default();
    keys.add(&ROUTE).unwrap();
    assert_eq!(Err(RegisterError::Duplicate("test:route".to_owned())), keys.add_layer_type::<Odd>());
}
