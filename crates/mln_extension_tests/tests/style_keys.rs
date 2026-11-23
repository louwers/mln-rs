//! Style keys declared through the facade: a map's style reads them, its
//! edits change them, a layer's renderer gets a change to its keys, and a
//! layer's update reads the root's.

mod support;

use std::sync::Arc;

use mln_rs::DemandMode;
use mln_rs::feature::Value;
use mln_rs::layer::{
    Convertible, Draws, EvaluateCx, Evaluation, Functions, Layer, LayerTypeInfo, NewCx, NoProperties, NoTiles,
    PrepareCx, SourceKind, StyleKey, StyleLayerType, UpdateCx, conversion,
};
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

/// A layer type whose layers keep the theme that their last update read.
struct Themed;

impl StyleLayerType for Themed {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "test-themed", source: SourceKind::None, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

struct ThemedLayer {
    theme: Option<String>,
}

impl Layer for ThemedLayer {
    type Style = Themed;
    type Input = NoTiles;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        ThemedLayer { theme: None }
    }

    fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::TRANSLUCENT
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        self.theme = cx.root::<Theme>().map(|theme| theme.0.clone());
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}

const STYLE: &str = r##"{"version":8,"test:theme":"night","sources":{},"layers":[
    {"id":"background","type":"background","test:group":"base","paint":{"background-color":"#ffffff"}},
    {"id":"themed","type":"test-themed"}]}"##;

fn fixture() -> Fixture {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_style_key(&THEME).unwrap();
    builder.add_style_key(&GROUP).unwrap();
    builder.add_style_key(&GROUP).unwrap();
    builder.add_layer_type::<ThemedLayer>().unwrap();
    Fixture::new(builder.build(), STYLE)
}

/// The theme that the layer `themed` read in its last update.
fn read_theme(fixture: &Fixture) -> Option<String> {
    wait(fixture.session.query_rendered_layer::<ThemedLayer, _>("themed", |layer, _| Ok(layer.theme.clone())))
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

#[test]
fn a_layers_update_reads_the_root_keys_of_the_frames_style() {
    let fixture = fixture();
    assert_eq!(Some("night".to_owned()), read_theme(&fixture));

    // The edit alone brings a frame, whose update reads the new value.
    wait(fixture.map.set_style_root_property("test:theme", r#""day""#));
    fixture.render_on(DemandMode::WaitForUpdate);
    assert_eq!(Some("day".to_owned()), read_theme(&fixture));

    wait(fixture.map.set_style_root_property("test:theme", "null"));
    fixture.render_on(DemandMode::WaitForUpdate);
    assert_eq!(None, read_theme(&fixture));
}
