//! Style keys declared through the facade: a map's style reads them, its
//! edits change them, and a layer's renderer gets a change to its keys.

mod support;

use std::sync::Arc;

use mln_rs::feature::Value;
use mln_rs::layer::{Convertible, Functions, StyleKey, conversion};
use mln_rs::mln_layers::BackgroundLayer;
use mln_rs::mln_render::RegistryBuilder;
use mln_rs::mln_util::ErrorKind;
use support::{Fixture, wait};

#[derive(Debug, PartialEq)]
struct Theme(String);

#[derive(Debug, PartialEq)]
struct Group(String);

static THEME: StyleKey<Theme> =
    StyleKey::root("test:theme", |value, cx| string(value, cx).map(Theme), |theme| Value::String(theme.0.clone()));
static GROUP: StyleKey<Group> =
    StyleKey::layer("test:group", |value, cx| string(value, cx).map(Group), |group| Value::String(group.0.clone()));

fn string(value: &Convertible<'_>, _: &Arc<Functions>) -> Result<String, conversion::Error> {
    value.to_string().ok_or_else(|| conversion::Error::new("expected a name"))
}

const STYLE: &str = r##"{"version":8,"test:theme":"night","sources":{},"layers":[
    {"id":"background","type":"background","test:group":"base","paint":{"background-color":"#ffffff"}}]}"##;

fn fixture() -> Fixture {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_style_key(&THEME).unwrap();
    builder.add_style_key(&GROUP).unwrap();
    builder.add_style_key(&GROUP).unwrap();
    Fixture::new(builder.build(), STYLE)
}

/// The group of the layer `background` as its renderer has it.
fn rendered_group(fixture: &Fixture) -> Option<String> {
    wait(fixture.session.query_rendered_layer::<BackgroundLayer, _>("background", |_, values| {
        Ok(values.style.extra::<Group>().map(|group| group.0.clone()))
    }))
}

#[test]
fn a_change_to_a_layers_keys_reaches_its_renderer() {
    let fixture = fixture();
    assert_eq!(Some("base".to_owned()), rendered_group(&fixture));

    wait(fixture.map.set_style_layer_property("background", "test:group", r#""top""#));
    fixture.render();
    assert_eq!(Some("top".to_owned()), rendered_group(&fixture));
    assert_eq!(Some(r#""top""#.to_owned()), wait(fixture.map.style_layer_property("background", "test:group")));

    wait(fixture.map.set_style_layer_property("background", "test:group", "null"));
    fixture.render();
    assert_eq!(None, rendered_group(&fixture));
}

#[test]
fn a_map_edits_the_keys_of_its_styles_root() {
    let fixture = fixture();
    assert_eq!(Some(r#""night""#.to_owned()), wait(fixture.map.style_root_property("test:theme")));
    wait(fixture.map.set_style_root_property("test:theme", r#""day""#));
    assert_eq!(Some(r#""day""#.to_owned()), wait(fixture.map.style_root_property("test:theme")));

    let error = fixture.map.set_style_root_property("test:themes", r#""day""#).unwrap().wait().unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
}
