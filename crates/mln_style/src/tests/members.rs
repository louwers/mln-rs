//! Extension members: a style keeps them, writes them back out and lets the
//! set calls change them; a registered member gives its value a type and
//! refuses what doesn't parse; one the registry lacks is kept as it is.

use std::sync::Arc;

use crate::StyleRegistry;
use crate::conversion::Error;
use crate::diff::{LayerChange, LayerChanges};
use crate::error::ErrorKind;
use crate::member::{ErasedMember, Member};
use crate::style::Style;
use crate::tests::util::FixtureLog;

#[derive(Debug, PartialEq)]
struct Theme(String);

#[derive(Debug, PartialEq)]
struct Group(String);

static THEME: Member<Theme> = Member::root("test:theme", |value, _| {
    value.to_string().map(Theme).ok_or_else(|| Error::new("expected a theme's name"))
});
static GROUP: Member<Group> = Member::layer("test:group", |value, _| {
    value.to_string().map(Group).ok_or_else(|| Error::new("expected a group's name"))
});

/// The built-ins and the test members.
fn registry() -> Arc<StyleRegistry> {
    let builtins = StyleRegistry::with_builtins();
    let members: [&'static dyn ErasedMember; 2] = [&THEME, &GROUP];
    Arc::new(StyleRegistry::new(
        Arc::clone(builtins.functions()),
        builtins.layer_types(),
        builtins.source_types(),
        members,
    ))
}

const STYLE: &str = r#"{"version":8,"test:theme":"night","other:root":{"a":[1,2.5]},"sources":{},"layers":[
    {"id":"paper","type":"background","test:group":"base","other:layer":true,"test:theme":"day"}]}"#;

fn style() -> Style {
    Style::from_json(&registry(), STYLE).unwrap()
}

fn layer_member(style: &Style, name: &str) -> Option<String> {
    style.layer_property_json("paper", name).unwrap()
}

fn root_member(style: &Style, name: &str) -> Option<String> {
    style.root_property_json(name).unwrap()
}

#[test]
fn a_style_keeps_its_members_and_writes_them_back() {
    let log = FixtureLog::new();
    let style = style();
    assert_eq!(Some(&Theme("night".to_owned())), style.members().get(&THEME));
    let paper = style.get_layer("paper").unwrap();
    assert_eq!(Some(&Group("base".to_owned())), paper.members().get(&GROUP));

    // Those the registry lacks at their place stay as they are.
    assert_eq!(Some(r#"{"a":[1,2.5]}"#.to_owned()), root_member(&style, "other:root"));
    assert_eq!(Some("true".to_owned()), layer_member(&style, "other:layer"));
    assert_eq!(Some(r#""day""#.to_owned()), layer_member(&style, "test:theme"));
    let warned = |message: &str| log.count(log::Level::Warn, "mln_style::parse_style", message);
    assert_eq!(1, warned(r#"no extension registered the member "other:root" of the style's root"#));
    assert_eq!(1, warned(r#"no extension registered the member "other:layer" of a layer"#));
    assert_eq!(1, warned(r#"no extension registered the member "test:theme" of a layer"#));

    // A layer's JSON holds its members, and adds back as the same layer.
    let json = style.layer_json("paper").unwrap();
    let mut copy = Style::from_json(&registry(), r#"{"version":8,"sources":{},"layers":[]}"#).unwrap();
    copy.add_layer_json(&json, None).unwrap();
    assert_eq!(paper.members(), copy.get_layer("paper").unwrap().members());
    assert_eq!(Some(&Group("base".to_owned())), copy.get_layer("paper").unwrap().members().get(&GROUP));
}

#[test]
fn a_registered_member_refuses_what_it_doesnt_parse() {
    let log = FixtureLog::new();
    let json =
        r#"{"version":8,"test:theme":1,"sources":{},"layers":[{"id":"paper","type":"background","test:group":2}]}"#;
    let mut style = Style::from_json(&registry(), json).unwrap();
    assert_eq!(None, root_member(&style, "test:theme"));
    assert_eq!(None, layer_member(&style, "test:group"));
    let warned = |message: &str| log.count(log::Level::Warn, "mln_style::parse_style", message);
    assert_eq!(1, warned(r#"member "test:theme": expected a theme's name"#));
    assert_eq!(1, warned(r#"layer 'paper': member "test:group": expected a group's name"#));

    let error = style.set_root_property_json("test:theme", "1").unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
    let error = style.set_layer_property_json("paper", "test:group", "[]").unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
    let error = style.add_layer_json(r#"{"id":"ink","type":"background","test:group":false}"#, None).unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
}

#[test]
fn the_set_calls_edit_members() {
    let _log = FixtureLog::new();
    let mut style = style();

    style.set_layer_property_json("paper", "test:group", r#""top""#).unwrap();
    assert_eq!(Some(&Group("top".to_owned())), style.get_layer("paper").unwrap().members().get(&GROUP));
    style.set_layer_property_json("paper", "other:layer", r#"{"b":null}"#).unwrap();
    assert_eq!(Some(r#"{"b":null}"#.to_owned()), layer_member(&style, "other:layer"));
    style.set_layer_property_json("paper", "test:group", "null").unwrap();
    assert_eq!(None, layer_member(&style, "test:group"));

    style.set_root_property_json("test:theme", r#""day""#).unwrap();
    assert_eq!(Some(&Theme("day".to_owned())), style.members().get(&THEME));
    style.set_root_property_json("other:root", "null").unwrap();
    assert_eq!(None, root_member(&style, "other:root"));
    style.set_root_property_json("new:root", "[1]").unwrap();
    assert_eq!(Some("[1]".to_owned()), root_member(&style, "new:root"));

    // Setting a member to the value it has changes nothing.
    let before = style.clone();
    style.set_root_property_json("test:theme", r#""day""#).unwrap();
    style.set_layer_property_json("paper", "other:layer", r#"{"b":null}"#).unwrap();
    assert!(before.is_same_version(&style));
}

#[test]
fn a_change_to_a_layers_members_restyles_it_without_a_layout() {
    let json = r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
        "layers":[{"id":"dots","type":"circle","source":"points","test:group":"base"}]}"#;
    let before = Style::from_json(&registry(), json).unwrap();
    let mut after = before.clone();
    after.set_layer_property_json("dots", "test:group", r#""top""#).unwrap();
    let changes = LayerChanges::between(before.get_layer_list(), after.get_layer_list());
    assert_eq!(Some(LayerChange::Restyled), changes.get("dots"));
}
