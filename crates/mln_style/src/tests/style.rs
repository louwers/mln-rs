use std::sync::Arc;
use std::time::Duration;

use super::util::{FixtureLog, new_layer, parse_json};
use crate::bitmap::PremultipliedImage;
use crate::color::Color;
use crate::conversion::Convertible;
use crate::expression::Functions;
use crate::feature::Value;
use crate::geo::LatLng;
use crate::layer::LayerStyle;
use crate::layers::Line;
use crate::projection::Projection;
use crate::property::transition::TransitionOptions;
use crate::property::value::{PropertyValue, StyleValue as _};
use crate::size::Size;
use crate::sky::Sky;
use crate::sources::tileset::vector_source as tileset_vector_source;
use crate::style::Style;
use crate::style::edit::Edit;
use crate::terrain::Terrain;
use crate::tileset::PartialTileset;
use crate::types::{ProjectionDefinition, VisibilityType};
use crate::{StyleRegistry, image};

/// An empty style.
fn stub_style() -> Style {
    Style::new(StyleRegistry::with_builtins())
}

/// Makes `edit`, which must succeed.
fn edit(style: &mut Style, edit: Edit) {
    style.apply(edit).unwrap();
}

/// Sets the global state value `name`.
fn set_state(style: &mut Style, name: &str, value: Value) {
    edit(style, Edit::SetGlobalState { name: name.to_owned(), value });
}

/// Sets the property `name` of the layer `layer` to `value`.
fn set_layer_property(
    style: &mut Style,
    layer: &str,
    name: &str,
    value: serde_json::Value,
) -> Result<(), crate::error::Error> {
    style.apply(Edit::SetLayerProperty { layer: layer.to_owned(), name: name.to_owned(), value })
}

// gtest: Style.Properties
#[test]
fn properties() {
    let mut style = stub_style();

    style.load_json(r#"{"name": "Test"}"#);
    assert_eq!("Test", style.get_name());

    style.load_json(r#"{"center": [10, 20]}"#);
    assert_eq!("", style.get_name());
    assert_eq!(LatLng::new(20.0, 10.0).unwrap(), style.get_default_camera().center.unwrap());

    style.load_json(r#"{"centerAltitude": 999}"#);
    assert_eq!("", style.get_name());
    assert_eq!(999.0, style.get_default_camera().center_altitude.unwrap());

    style.load_json(r#"{"bearing": 24}"#);
    assert_eq!("", style.get_name());
    assert_eq!(LatLng::default(), style.get_default_camera().center.unwrap());
    assert_eq!(24.0, style.get_default_camera().bearing.unwrap());

    style.load_json(r#"{"zoom": 13.3}"#);
    assert_eq!("", style.get_name());
    assert_eq!(13.3, style.get_default_camera().zoom.unwrap());

    style.load_json(r#"{"pitch": 60}"#);
    assert_eq!("", style.get_name());
    assert_eq!(60.0, style.get_default_camera().pitch.unwrap());

    style.load_json(r#"{"roll": 99}"#);
    assert_eq!("", style.get_name());
    assert_eq!(99.0, style.get_default_camera().roll.unwrap());

    style.load_json(r"{}");
    assert_eq!(Duration::from_millis(300), style.get_transition_options().duration.unwrap());
    assert_eq!(None, style.get_transition_options().delay);

    style.load_json(r#"{"transition": { "duration": 500, "delay": 50 }}"#);
    assert_eq!(Duration::from_millis(500), style.get_transition_options().duration.unwrap());
    assert_eq!(Duration::from_millis(50), style.get_transition_options().delay.unwrap());

    style.load_json(r#"{"name": 23, "center": {}, "bearing": "north", "zoom": null, "pitch": "wide"}"#);
    assert_eq!("", style.get_name());
    assert_eq!(LatLng::default(), style.get_default_camera().center.unwrap());
    assert_eq!(0.0, style.get_default_camera().zoom.unwrap());
    assert_eq!(0.0, style.get_default_camera().bearing.unwrap());
    assert_eq!(0.0, style.get_default_camera().pitch.unwrap());
}

// gtest: Style.Projection
#[test]
fn projection() {
    let mut style = stub_style();

    style.load_json(r#"{"projection": {"type": "globe"}}"#);
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant().unwrap());

    style.load_json(r"{}");
    assert!(style.get_projection().get_type().is_undefined());

    let mut projection = Projection::new();
    projection.set_type(PropertyValue::Constant(ProjectionDefinition::from_name("vertical-perspective")));
    edit(&mut style, Edit::SetProjection(projection));
    assert_eq!(
        ProjectionDefinition::from_name("vertical-perspective"),
        *style.get_projection().get_type().as_constant().unwrap()
    );

    style.load_json(r#"{"projection": {"type": "EPSG:3413"}}"#);
    assert_eq!(Some("EPSG:3413"), style.get_projection().registered_name());
    assert!(style.get_projection().get_type().is_undefined());

    // A name that is neither built in nor a registered projection's keeps the
    // default, with a warning.
    style.load_json(r#"{"projection": {"type": "orthographic"}}"#);
    assert!(style.get_projection().get_type().is_undefined());
    assert_eq!(None, style.get_projection().registered_name());
}

fn make_image(id: &str) -> Arc<image::Impl> {
    Arc::new(
        image::Impl::new(
            id.to_owned(),
            PremultipliedImage::new(Size::new(16, 16)),
            2.0,
            false,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .unwrap(),
    )
}

// gtest: Style.AddRemoveImage
#[test]
fn add_remove_image() {
    let mut style = stub_style();
    for id in ["one", "two", "three"] {
        edit(&mut style, Edit::AddImage(make_image(id)));
    }

    edit(&mut style, Edit::RemoveImage("one".to_owned()));
    edit(&mut style, Edit::RemoveImage("two".to_owned()));

    assert!(style.get_image("three").is_some());
    assert!(style.get_image("two").is_none());
    assert!(style.get_image("four").is_none());
}

// gtest: Style.AddRemoveRemoveImage
#[test]
fn add_remove_remove_image() {
    // regression test for https://github.com/mapbox/mapbox-gl-native/pull/16391
    let mut style = stub_style();
    for id in ["one", "two", "three"] {
        edit(&mut style, Edit::AddImage(make_image(id)));
    }

    edit(&mut style, Edit::RemoveImage("one".to_owned()));
    edit(&mut style, Edit::RemoveImage("two".to_owned()));
    edit(&mut style, Edit::RemoveImage("two".to_owned()));

    assert!(style.get_image("three").is_some());
    assert!(style.get_image("two").is_none());
    assert!(style.get_image("four").is_none());
}

// gtest: Style.LayerTransitionRefreshesImmutableCollection
#[test]
fn layer_transition_refreshes_immutable_collection() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    edit(&mut style, Edit::AddLayer { layer: new_layer("line", "line", "source"), before: None });
    let before = Arc::clone(style.get_layer_list());

    let transition = serde_json::json!({"duration": 123, "delay": 45});
    set_layer_property(&mut style, "line", "line-color-transition", transition).unwrap();
    let after = Arc::clone(style.get_layer_list());
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(1, after.len());
    let impl_ = LayerStyle::<Line>::recover_ref(&**after.first().unwrap().style()).unwrap();
    assert_eq!(Some(Duration::from_millis(123)), impl_.paint.line_color.options.duration);
    assert_eq!(Some(Duration::from_millis(45)), impl_.paint.line_color.options.delay);
}

// gtest: Style.ImageRemovalPublishesUpdates
#[test]
fn image_removal_changes_the_images() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    edit(&mut style, Edit::AddImage(make_image("image")));
    assert_eq!(1, style.get_images().len());

    let before = style.clone();
    edit(&mut style, Edit::RemoveImage("image".to_owned()));
    assert_eq!(["images"], *parts_that_differ(&before, &style));
    assert!(style.get_images().is_empty());
}

#[test]
fn is_supported_script_of_a_literal_in_a_layout_property() {
    // Not a C++ test: ffi patch 0093 kept "is-supported-script" from folding,
    // so this text-field wasn't a literal and property conversion rejected it
    // (an assertion in debug builds). The port lets it fold (see
    // REQUIRED_PROPS in expression/parse.rs).
    let mut style = stub_style();
    style.load_json(
        r#"{"version":8,"sources":{"s":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
        "layers":[{"id":"symbol","type":"symbol","source":"s",
        "layout":{"text-field":["case",["is-supported-script","မြန်မာ"],"local","fallback"]}}]}"#,
    );
    assert!(style.get_layer("symbol").is_some());
}

// gtest: Style.TransitionOptionsPublishUpdates
#[test]
fn transition_options_change_the_transition() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    let transition = |placement| {
        TransitionOptions::new(Some(Duration::from_millis(123)), Some(Duration::from_millis(45)), placement)
    };

    let before = style.clone();
    edit(&mut style, Edit::SetTransition(transition(true)));
    assert_eq!(Some(Duration::from_millis(123)), style.get_transition_options().duration);
    assert_eq!(Some(Duration::from_millis(45)), style.get_transition_options().delay);
    assert_eq!(["transition"], *parts_that_differ(&before, &style));

    let before = style.clone();
    edit(&mut style, Edit::SetTransition(transition(false)));
    assert!(!style.get_transition_options().enable_placement_transitions);
    assert_eq!(["transition"], *parts_that_differ(&before, &style));

    let before = style.clone();
    edit(&mut style, Edit::SetTransition(transition(false)));
    assert!(before.is_same_version(&style));
}

#[test]
fn a_layer_edit_that_changes_nothing_keeps_the_layers() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background"}]}"#);

    let before = Arc::clone(style.get_layer_list());
    let same = style.get_layer("bg").unwrap().clone();
    edit(&mut style, Edit::SetLayer(same));
    assert!(Arc::ptr_eq(&before, style.get_layer_list()));

    set_layer_property(&mut style, "bg", "background-color", serde_json::json!("white")).unwrap();
    assert!(!Arc::ptr_eq(&before, style.get_layer_list()));

    let error = set_layer_property(&mut style, "absent", "background-color", serde_json::json!("white")).unwrap_err();
    assert_eq!(crate::error::ErrorKind::NotFound, error.kind);
}

#[test]
fn layer_changes_leave_earlier_versions_unchanged() {
    let mut style = stub_style();
    style.load_json(
        r#"{"version":8,"sources":{},"layers":[{"id":"a","type":"background"},{"id":"b","type":"background"}]}"#,
    );
    let ids = |layers: &crate::LayerList| layers.iter().map(|layer| layer.get_id().to_owned()).collect::<Vec<_>>();
    let loaded = style.clone();
    assert_eq!(["a", "b"], *ids(loaded.get_layer_list()));

    edit(&mut style, Edit::AddLayer { layer: new_layer("line", "c", "source"), before: Some("b".to_owned()) });
    let error = style
        .apply(Edit::AddLayer { layer: new_layer("line", "a", "source"), before: Some("b".to_owned()) })
        .unwrap_err();
    assert_eq!(crate::error::ErrorKind::AlreadyExists, error.kind);
    assert_eq!("Layer a already exists", error.message);
    let added = style.clone();
    assert_eq!(["a", "c", "b"], *ids(added.get_layer_list()));
    assert_eq!(["layers"], *parts_that_differ(&loaded, &added));

    edit(&mut style, Edit::RemoveLayer("a".to_owned()));
    assert_eq!(["c", "b"], *ids(style.get_layer_list()));
    assert_eq!(["a", "b"], *ids(loaded.get_layer_list()));
    assert_eq!(["a", "c", "b"], *ids(added.get_layer_list()));
    let error = style.apply(Edit::RemoveLayer("a".to_owned())).unwrap_err();
    assert_eq!(crate::error::ErrorKind::NotFound, error.kind);
}

// gtest: Style.GlobalState
#[test]
fn global_state() {
    let mut style = stub_style();

    // Defaults from the root "state" property are used as the initial global state.
    style.load_json(
        r#"{
        "version": 8,
        "state": {
            "showLabels": {"default": true},
            "categories": {"default": ["restaurant", "hotel"]},
            "noDefault": {}
        },
        "sources": {},
        "layers": []
    }"#,
    );

    let state = style.get_global_state();
    assert_eq!(2, state.len());
    assert_eq!(Value::Bool(true), state["showLabels"]);
    assert_eq!(
        Value::Array(vec![Value::String("restaurant".into()), Value::String("hotel".into())]),
        state["categories"]
    );
    assert!(!state.contains_key("noDefault"));

    // Setting a property updates the state.
    set_state(&mut style, "showLabels", Value::Bool(false));
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);

    // Properties that are not defined in the style can also be set.
    set_state(&mut style, "custom", Value::Double(42.0));
    assert_eq!(Value::Double(42.0), style.get_global_state()["custom"]);

    // A null value resets the property to its default.
    set_state(&mut style, "showLabels", Value::Null);
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);

    // A null value on a property without default resets to null.
    set_state(&mut style, "custom", Value::Null);
    assert_eq!(Value::Null, style.get_global_state()["custom"]);

    // Loading a new style resets the global state.
    style.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    assert!(style.get_global_state().is_empty());

    // As in GL JS, setting an absent property to null creates an observable
    // null-valued entry even though both forms evaluate to null in expressions.
    let before_null = Arc::clone(style.get_global_state());
    set_state(&mut style, "neverSet", Value::Null);
    assert!(!Arc::ptr_eq(&before_null, style.get_global_state()));
    let state = style.get_global_state();
    assert_eq!(1, state.len());
    assert_eq!(Value::Null, state["neverSet"]);

    // Repeating the same null assignment remains a no-op.
    let after_null = Arc::clone(style.get_global_state());
    set_state(&mut style, "neverSet", Value::Null);
    assert!(Arc::ptr_eq(&after_null, style.get_global_state()));
}

// gtest: Style.GlobalStateNumericEquality
#[test]
fn global_state_numeric_equality() {
    let mut style = stub_style();

    style.load_json(
        r#"{
        "version": 8,
        "state": {"minSpeed": {"default": 50}},
        "sources": {},
        "layers": []
    }"#,
    );

    // Setting the same numeric value with a different arithmetic type
    // (integer default vs. runtime double) must be treated as unchanged.
    let before = Arc::clone(style.get_global_state());
    set_state(&mut style, "minSpeed", Value::Double(50.0));
    assert!(Arc::ptr_eq(&before, style.get_global_state()));

    set_state(&mut style, "minSpeed", Value::Double(51.0));
    assert!(!Arc::ptr_eq(&before, style.get_global_state()));
}

// gtest: Style.GlobalStateVisibility
#[test]
fn global_state_visibility() {
    let log = FixtureLog::new();

    let mut style = stub_style();

    style.load_json(
        r#"{
        "version": 8,
        "state": {"showBackground": {"default": true}},
        "sources": {},
        "layers": [{
            "id": "background",
            "type": "background",
            "layout": {
                "visibility": ["case", ["to-boolean", ["global-state", "showBackground"]], "visible", "none"]
            }
        }]
    }"#,
    );

    assert!(style.get_layer("background").is_some());
    // Parsing the layer happens before it has access to the style's global
    // state, but a valid expression must not emit a spurious warning.
    assert!(log.is_empty(), "{:?}", log.messages());
    // The default from the root "state" property applies.
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    set_state(&mut style, "showBackground", Value::Bool(false));
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());

    set_state(&mut style, "showBackground", Value::Null);
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    // Setting a constant visibility replaces the expression: further state
    // changes no longer apply.
    edit(&mut style, Edit::SetLayerVisibility { layer: "background".to_owned(), visibility: VisibilityType::None });
    set_state(&mut style, "showBackground", Value::Bool(true));
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());
}

// gtest: Style.VisibilityExpressionSetAtRuntime
#[test]
fn visibility_expression_set_at_runtime() {
    let log = FixtureLog::new();

    let mut style = stub_style();

    style.load_json(
        r#"{
        "version": 8,
        "state": {"showBackground": {"default": false}},
        "sources": {},
        "layers": [{"id": "background", "type": "background"}]
    }"#,
    );

    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    // A visibility expression set after the style has loaded must be
    // evaluated against the current global state right away.
    let json = serde_json::json!(["case", ["to-boolean", ["global-state", "showBackground"]], "visible", "none"]);
    set_layer_property(&mut style, "background", "visibility", json).unwrap();
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());

    // Runtime global state (not only style defaults) applies as well.
    set_state(&mut style, "showBackground", Value::Bool(true));
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    let json = serde_json::json!(["case", ["to-boolean", ["global-state", "showBackground"]], "none", "visible"]);
    set_layer_property(&mut style, "background", "visibility", json).unwrap();
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());
    assert!(log.is_empty(), "{:?}", log.messages());
}

// gtest: Style.LayerAddedAfterGlobalStateUpdateUsesCurrentState
#[test]
fn layer_added_after_global_state_update_uses_current_state() {
    let mut style = stub_style();
    style.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    set_state(&mut style, "showLayer", Value::Bool(false));

    let mut layer = new_layer("line", "line", "source");
    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showLayer"]], "visible", "none"]"#);
    assert!(layer.set_property("visibility", &Convertible::from(&json), None).is_none());
    assert_eq!(VisibilityType::Visible, layer.get_visibility());

    edit(&mut style, Edit::AddLayer { layer, before: None });
    assert_eq!(VisibilityType::None, style.get_layer("line").unwrap().get_visibility());
}

// gtest: Style.VisibilityExpressionRejectsOtherDependencies
#[test]
fn visibility_expression_rejects_other_dependencies() {
    let mut style = stub_style();

    style.load_json(
        r#"{
        "version": 8,
        "sources": {},
        "layers": [{"id": "background", "type": "background"}]
    }"#,
    );

    // Visibility expressions may only depend on the global state.
    let json = serde_json::json!(["case", [">", ["zoom"], 10], "visible", "none"]);
    let error = set_layer_property(&mut style, "background", "visibility", json).unwrap_err();
    assert_eq!(crate::error::ErrorKind::InvalidArgument, error.kind);
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());
}

/// gtest `EXPECT_FLOAT_EQ`: within 4 ULPs.
fn assert_float_eq(expected: f32, actual: f32) {
    let diff = (i64::from(expected.to_bits()) - i64::from(actual.to_bits())).abs();
    assert!(diff <= 4, "expected {expected}, got {actual}");
}

// gtest: Style.SkyPresenceAndReplacement
#[test]
fn sky_presence_and_replacement() {
    let mut style = stub_style();

    assert!(style.get_sky().is_none());

    style.load_json(r#"{"sky":{}}"#);
    assert!(style.get_sky().expect("sky").properties.sky_color.value.is_undefined());

    style.load_json(r#"{"sky":{"sky-color":"blue","atmosphere-blend":0.4}}"#);
    let sky = style.get_sky().expect("sky");
    assert_eq!(Color::blue(), *sky.properties.sky_color.value.as_constant().unwrap());
    assert_float_eq(0.4, *sky.properties.sky_atmosphere_blend.value.as_constant().unwrap());

    let mut replacement = Sky::new();
    assert!(
        replacement
            .set_property("fog-color", &Convertible::from(&serde_json::json!("red")), &Functions::with_builtins())
            .is_none()
    );
    edit(&mut style, Edit::SetSky(Some(replacement)));
    let sky = style.get_sky().expect("sky");
    assert!(sky.properties.sky_color.value.is_undefined());
    assert!(sky.properties.sky_atmosphere_blend.value.is_undefined());
    assert_eq!(Color::red(), *sky.properties.sky_fog_color.value.as_constant().unwrap());

    edit(&mut style, Edit::SetSky(None));
    assert!(style.get_sky().is_none());

    style.load_json(r#"{"sky":{"sky-color":"white"}}"#);
    assert!(style.get_sky().is_some());
    style.load_json(r"{}");
    assert!(style.get_sky().is_none());

    style.load_json(r#"{"sky":[]}"#);
    assert!(style.get_sky().is_none());
}

const TERRAIN_STYLE: &str = r#"{"version":8,"sources":{
    "dem":{"type":"raster-dem","tiles":["https://example.com/{z}/{x}/{y}.png"]},
    "points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#;

/// `TERRAIN_STYLE` with the root `terrain` set to the JSON `terrain`.
fn terrain_style(terrain: &str) -> String {
    TERRAIN_STYLE.replace(r#""layers":[]"#, &format!(r#""layers":[],"terrain":{terrain}"#))
}

fn terrain_of(style: &Style) -> Option<(&str, f32)> {
    style.get_terrain().map(|terrain| (terrain.source.as_str(), terrain.exaggeration))
}

#[test]
fn the_terrain_is_parsed_set_replaced_and_cleared() {
    let mut style = stub_style();
    style.load_json(TERRAIN_STYLE);
    assert!(style.get_terrain().is_none());

    style.load_json(&terrain_style(r#"{"source":"dem","exaggeration":3}"#));
    assert_eq!(terrain_of(&style), Some(("dem", 3.0)));
    style.load_json(&terrain_style(r#"{"source":"dem"}"#));
    assert_eq!(terrain_of(&style), Some(("dem", 1.0)));
    style.load_json(TERRAIN_STYLE);

    edit(&mut style, Edit::SetTerrain(Some(Terrain::new("dem"))));
    assert_eq!(terrain_of(&style), Some(("dem", 1.0)));
    edit(&mut style, Edit::SetTerrain(Some(Terrain { source: "dem".to_owned(), exaggeration: 0.0 })));
    assert_eq!(terrain_of(&style), Some(("dem", 0.0)));
    edit(&mut style, Edit::SetTerrain(None));
    assert!(style.get_terrain().is_none());
}

#[test]
fn an_edit_that_sets_the_terrain_it_has_changes_nothing() {
    let mut style = stub_style();
    style.load_json(TERRAIN_STYLE);
    edit(&mut style, Edit::SetTerrain(Some(Terrain::new("dem"))));

    let before = style.clone();
    edit(&mut style, Edit::SetTerrain(Some(Terrain::new("dem"))));
    assert!(before.is_same_version(&style));

    edit(&mut style, Edit::SetTerrain(Some(Terrain { source: "dem".to_owned(), exaggeration: 2.0 })));
    assert!(!before.is_same_version(&style));
    assert_eq!(["terrain"], *parts_that_differ(&before, &style));

    edit(&mut style, Edit::SetTerrain(None));
    let before = style.clone();
    edit(&mut style, Edit::SetTerrain(None));
    assert!(before.is_same_version(&style));
}

#[test]
fn a_terrain_needs_a_raster_dem_source_if_the_style_has_the_source() {
    let mut style = stub_style();
    style.load_json(TERRAIN_STYLE);

    let before = style.clone();
    let error = style.apply(Edit::SetTerrain(Some(Terrain::new("points")))).unwrap_err();
    assert_eq!(crate::error::ErrorKind::InvalidArgument, error.kind);
    assert!(before.is_same_version(&style));

    // A source the style lacks is accepted: the terrain waits for it.
    edit(&mut style, Edit::SetTerrain(Some(Terrain::new("absent"))));
    assert_eq!(terrain_of(&style), Some(("absent", 1.0)));
}

#[test]
fn a_terrains_exaggeration_is_finite_and_at_least_zero() {
    let mut style = stub_style();
    style.load_json(TERRAIN_STYLE);

    for exaggeration in [-1.0, f32::NAN, f32::INFINITY] {
        let terrain = Terrain { source: "dem".to_owned(), exaggeration };
        let error = style.apply(Edit::SetTerrain(Some(terrain))).unwrap_err();
        assert_eq!(crate::error::ErrorKind::InvalidArgument, error.kind, "{exaggeration}");
    }
    assert!(style.get_terrain().is_none());

    // A number too large for an f32.
    style.load_json(&terrain_style(r#"{"source":"dem","exaggeration":1e40}"#));
    assert!(style.get_terrain().is_none());
}

#[test]
fn a_parsed_terrain_that_doesnt_fit_is_dropped() {
    let mut style = stub_style();

    let log = FixtureLog::new();
    style.load_json(&terrain_style(r#"{"source":"points"}"#));
    assert!(style.get_terrain().is_none());
    assert!(log.messages().iter().any(|message| message.msg.contains("not a raster-dem source")));

    for terrain in [r#"{"source":"absent"}"#, r#"{"source":"dem","exaggeration":2}"#] {
        style.load_json(&terrain_style(terrain));
        assert!(style.get_terrain().is_some(), "{terrain}");
    }
    for (terrain, message) in [
        ("{}", "terrain must have a source"),
        (r#"{"source":1}"#, "terrain source must be a string"),
        (r#"{"source":"dem","exaggeration":"high"}"#, "terrain exaggeration must be a number"),
        (r#"{"source":"dem","exaggeration":-1}"#, "terrain exaggeration must be at least 0"),
        ("[]", "terrain must be an object"),
        ("null", "terrain must be an object"),
    ] {
        style.load_json(&terrain_style(terrain));
        assert!(style.get_terrain().is_none(), "{terrain}");
        assert!(log.messages().iter().any(|logged| logged.msg.contains(message)), "{terrain}");
    }
}

#[test]
fn removing_the_terrains_source_keeps_the_terrain() {
    // As in GL JS, where only a layer stops a source from being removed: the
    // terrain stays, and shows nothing until a source with its id is back.
    let mut style = stub_style();
    style.load_json(TERRAIN_STYLE);
    edit(&mut style, Edit::SetTerrain(Some(Terrain::new("dem"))));

    edit(&mut style, Edit::RemoveSource("dem".to_owned()));
    assert_eq!(terrain_of(&style), Some(("dem", 1.0)));
}

// gtest: Style.SkyProjectionAndGlobalStateCoexist
#[test]
fn sky_projection_and_global_state_coexist() {
    let mut style = stub_style();

    style.load_json(
        r#"{
        "version": 8,
        "sky": {"sky-color": "blue"},
        "projection": {"type": "globe"},
        "state": {"showLabels": {"default": true}},
        "sources": {},
        "layers": []
    }"#,
    );

    assert_eq!(Color::blue(), *style.get_sky().expect("sky").properties.sky_color.value.as_constant().unwrap());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant().unwrap());
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);

    set_state(&mut style, "showLabels", Value::Bool(false));
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
    assert_eq!(Color::blue(), *style.get_sky().expect("sky").properties.sky_color.value.as_constant().unwrap());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant().unwrap());

    edit(&mut style, Edit::SetSky(None));
    assert!(style.get_sky().is_none());
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
}

fn vector_source(id: &str) -> crate::Source {
    tileset_vector_source(id, Some("mptiler://tiles/contours".to_owned()), PartialTileset::default())
}

#[test]
fn sources_keep_the_order_they_were_added_in() {
    let mut style = stub_style();
    for id in ["c", "b", "a"] {
        edit(&mut style, Edit::AddSource(vector_source(id)));
    }
    assert_eq!(["c", "b", "a"], *ids_of(style.get_sources()));
    let error = style.apply(Edit::AddSource(vector_source("b"))).unwrap_err();
    assert_eq!(crate::error::ErrorKind::AlreadyExists, error.kind);

    // A changed source keeps its place.
    let before = style.clone();
    let mut common = *style.get_source("b").unwrap().common();
    common.volatile = true;
    edit(&mut style, Edit::SetSourceCommon { id: "b".to_owned(), common });
    assert_eq!(["c", "b", "a"], *ids_of(style.get_sources()));
    assert!(style.get_source("b").unwrap().common().volatile);
    assert_eq!(["sources"], *parts_that_differ(&before, &style));
    // The same members change nothing.
    let after = style.clone();
    edit(&mut style, Edit::SetSourceCommon { id: "b".to_owned(), common });
    assert!(after.is_same_version(&style));

    edit(&mut style, Edit::RemoveSource("b".to_owned()));
    assert_eq!(["c", "a"], *ids_of(style.get_sources()));
    assert_eq!(["c", "b", "a"], *ids_of(before.get_sources()));
    let error = style.apply(Edit::RemoveSource("b".to_owned())).unwrap_err();
    assert_eq!(crate::error::ErrorKind::NotFound, error.kind);
}

/// A style with the raster DEM source `dem`, the GeoJSON source `points`,
/// and the circle layer `dots` on `points`.
const DOTS_STYLE: &str = r#"{"version":8,"sources":{
    "dem":{"type":"raster-dem","tiles":["https://example.com/{z}/{x}/{y}.png"]},
    "points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},
    "layers":[{"id":"dots","type":"circle","source":"points"}]}"#;

#[track_caller]
fn assert_incompatible(style: &mut Style, edit: Edit) {
    let before = style.clone();
    let error = style.apply(edit).unwrap_err();
    assert_eq!(crate::error::ErrorKind::InvalidArgument, error.kind, "{error}");
    assert!(error.to_string().contains("not compatible"), "{error}");
    assert!(before.is_same_version(style));
}

#[test]
fn a_replaced_layer_needs_a_source_it_can_draw() {
    let mut style = stub_style();
    style.load_json(DOTS_STYLE);
    assert_incompatible(&mut style, Edit::SetLayer(new_layer("circle", "dots", "dem")));
    edit(&mut style, Edit::SetLayer(new_layer("hillshade", "dots", "dem")));
}

#[test]
fn a_layers_source_property_needs_a_source_it_can_draw() {
    let mut style = stub_style();
    style.load_json(DOTS_STYLE);
    let source = |id: &str| Edit::SetLayerProperty {
        layer: "dots".to_owned(),
        name: "source".to_owned(),
        value: serde_json::json!(id),
    };
    assert_incompatible(&mut style, source("dem"));
    edit(&mut style, source("absent"));
}

#[test]
fn an_added_source_needs_to_feed_the_layers_that_name_it() {
    let mut style = stub_style();
    style.load_json(DOTS_STYLE);
    edit(&mut style, Edit::AddLayer { layer: new_layer("fill", "waiting", "s"), before: None });
    let raster = || crate::sources::tileset::raster_source("s", None, PartialTileset::default(), 256);
    assert_incompatible(&mut style, Edit::AddSource(raster()));
    edit(&mut style, Edit::RemoveLayer("waiting".to_owned()));
    edit(&mut style, Edit::AddSource(raster()));
}

#[test]
fn an_edit_replaces_only_the_part_it_changes() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background"}]}"#);
    let mut case = |edit_to_make: Edit, parts: &[&str]| {
        let before = style.clone();
        edit(&mut style, edit_to_make);
        assert_eq!(parts, parts_that_differ(&before, &style));
    };
    case(Edit::SetLayerZoomRange { layer: "bg".to_owned(), min: 1.0, max: 2.0 }, &["layers"]);
    case(Edit::AddSource(vector_source("s")), &["sources"]);
    case(Edit::AddImage(make_image("i")), &["images"]);
    case(Edit::SetLight(crate::light::Light::new()), &["light"]);
    case(Edit::SetSky(Some(Sky::new())), &["sky"]);
    case(Edit::SetTerrain(Some(Terrain::new("absent"))), &["terrain"]);
    case(Edit::SetProjection(Projection::new()), &["projection"]);
    case(Edit::SetTransition(TransitionOptions::new(Some(Duration::from_millis(1)), None, true)), &["transition"]);
    case(Edit::SetGlobalState { name: "k".to_owned(), value: Value::Bool(true) }, &["global state"]);
    case(
        Edit::SetGlobalStateDefaults(crate::expression::GlobalStateMap::new()),
        &["global state", "global state defaults"],
    );
    let other = Style::from_json(style.registry(), r#"{"version":8,"name":"other","sources":{},"layers":[]}"#).unwrap();
    assert!(!style.is_same_version(&other));
    assert!(style.is_same_version(&style.clone()));
}

/// The parts of `a` and `b` whose `Arc`s differ, and the transition if it
/// differs.
fn parts_that_differ(a: &Style, b: &Style) -> Vec<&'static str> {
    let sky = |style: &Style| style.get_sky().map(Arc::as_ptr);
    let terrain = |style: &Style| style.get_terrain().map(Arc::as_ptr);
    [
        ("layers", Arc::ptr_eq(a.get_layer_list(), b.get_layer_list())),
        ("sources", Arc::ptr_eq(a.get_sources(), b.get_sources())),
        ("images", Arc::ptr_eq(a.get_images(), b.get_images())),
        ("light", Arc::ptr_eq(a.get_light(), b.get_light())),
        ("sky", sky(a) == sky(b)),
        ("terrain", terrain(a) == terrain(b)),
        ("projection", Arc::ptr_eq(a.get_projection(), b.get_projection())),
        ("transition", a.get_transition_options() == b.get_transition_options()),
        ("global state", Arc::ptr_eq(a.get_global_state(), b.get_global_state())),
        ("global state defaults", Arc::ptr_eq(a.get_global_state_defaults(), b.get_global_state_defaults())),
    ]
    .into_iter()
    .filter_map(|(part, same)| (!same).then_some(part))
    .collect()
}

fn ids_of(sources: &crate::SourceList) -> Vec<String> {
    sources.iter().map(|source| source.id().to_owned()).collect()
}
