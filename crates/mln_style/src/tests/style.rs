use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use feature::Value;
use mln_conversion::Convertible;
use mln_style_types::{ProjectionDefinition, VisibilityType};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::tileset::PartialTileset;
use mln_util::{Color, LatLng};

use super::util::{FixtureLog, new_layer, parse_json};
use crate::expression::Functions;
use crate::layer_type::LayerStyle;
use crate::layers::Line;
use crate::observer::Observer;
use crate::projection::Projection;
use crate::property_value::PropertyValue;
use crate::sky::Sky;
use crate::source::Source as _;
use crate::sources::tile_source::TileSource;
use crate::style::{Impl, Style};
use crate::transition_options::TransitionOptions;
use crate::{StyleRegistry, image};

/// An empty style impl.
fn stub_style() -> Impl {
    Impl::new(StyleRegistry::with_builtins())
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
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant());

    style.load_json(r"{}");
    assert!(style.get_projection().get_type().is_undefined());

    let mut projection = Projection::new();
    projection.set_type(PropertyValue::Constant(ProjectionDefinition::from_name("vertical-perspective")));
    style.set_projection(projection);
    assert_eq!(
        ProjectionDefinition::from_name("vertical-perspective"),
        *style.get_projection().get_type().as_constant()
    );

    // A name that is not a projection keeps the default, with a warning.
    style.load_json(r#"{"projection": {"type": "orthographic"}}"#);
    assert!(style.get_projection().get_type().is_undefined());
}

// gtest: Style.GlobalStatePropertyRequiresLoadedStyle
#[test]
fn global_state_property_requires_loaded_style() {
    let mut style = Style { impl_: Box::new(stub_style()) };

    assert!(!style.is_loaded());
    assert!(style.set_global_state_property("showLabels", &Value::Bool(false)).is_err());
    assert!(style.get_global_state().is_empty());

    style.load_json(
        r#"{
        "version": 8,
        "state": {"showLabels": {"default": true}},
        "sources": {},
        "layers": []
    }"#,
    );

    assert!(style.is_loaded());
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);
    assert!(style.set_global_state_property("showLabels", &Value::Bool(false)).is_ok());
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
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
    style.add_image(make_image("one"));
    style.add_image(make_image("two"));
    style.add_image(make_image("three"));

    style.remove_image("one");
    style.remove_image("two");

    assert!(style.get_image("three").is_some());
    assert!(style.get_image("two").is_none());
    assert!(style.get_image("four").is_none());
}

// gtest: Style.AddRemoveRemoveImage
#[test]
fn add_remove_remove_image() {
    // regression test for https://github.com/mapbox/mapbox-gl-native/pull/16391
    let mut style = stub_style();
    style.add_image(make_image("one"));
    style.add_image(make_image("two"));
    style.add_image(make_image("three"));

    style.remove_image("one");
    style.remove_image("two");
    style.remove_image("two");

    assert!(style.get_image("three").is_some());
    assert!(style.get_image("two").is_none());
    assert!(style.get_image("four").is_none());
}

// gtest: Style.LayerTransitionRefreshesImmutableCollection
#[test]
fn layer_transition_refreshes_immutable_collection() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    style.add_layer(new_layer("line", "line", "source"), None).unwrap();
    let before = Arc::clone(style.get_layer_list());

    let transition = serde_json::json!({"duration": 123, "delay": 45});
    let error = style.update_layer("line", |layer| {
        layer.set_property("line-color-transition", &Convertible::from(&transition), None)
    });
    assert!(error.unwrap().is_none());
    let after = Arc::clone(style.get_layer_list());
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(1, after.len());
    let impl_ = LayerStyle::<Line>::recover_ref(&**after.first().unwrap().style()).unwrap();
    assert_eq!(Some(Duration::from_millis(123)), impl_.paint.line_color.options.duration);
    assert_eq!(Some(Duration::from_millis(45)), impl_.paint.line_color.options.delay);
}

#[derive(Default)]
struct CountingObserver {
    updates: Cell<usize>,
}

impl Observer for CountingObserver {
    fn on_update(&self) {
        self.updates.set(self.updates.get() + 1);
    }
}

// gtest: Style.ImageRemovalPublishesUpdates
#[test]
fn image_removal_publishes_updates() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    style.add_image(make_image("image"));
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(Rc::<CountingObserver>::clone(&observer)));
    assert_eq!(1, style.get_image_impls().len());

    style.remove_image("image");
    assert_eq!(1, observer.updates.get());
    assert!(style.get_image_impls().is_empty());
}

#[test]
fn is_supported_script_of_a_literal_in_a_layout_property() {
    // Not a C++ test: ffi patch 0093 kept "is-supported-script" from folding,
    // so this text-field wasn't a literal and property conversion rejected it
    // (an assertion in debug builds). The port lets it fold (see
    // REQUIRED_PROPS in mln_expression's parsing_context.rs).
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
fn transition_options_publish_updates() {
    // The C++ observer reads the style back inside onUpdate; a Rust observer cannot borrow the
    // style it observes, so this reads it after each call.
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(Rc::<CountingObserver>::clone(&observer)));

    style.set_transition_options(&TransitionOptions::new(
        Some(Duration::from_millis(123)),
        Some(Duration::from_millis(45)),
        true,
    ));
    assert_eq!(Some(Duration::from_millis(123)), style.get_transition_options().duration);
    assert_eq!(Some(Duration::from_millis(45)), style.get_transition_options().delay);
    assert_eq!(1, observer.updates.get());
    style.set_transition_options(&TransitionOptions::new(
        Some(Duration::from_millis(123)),
        Some(Duration::from_millis(45)),
        false,
    ));
    assert!(!style.get_transition_options().enable_placement_transitions);
    assert_eq!(2, observer.updates.get());

    let updates = observer.updates.get();
    style.set_transition_options(&TransitionOptions::new(
        Some(Duration::from_millis(123)),
        Some(Duration::from_millis(45)),
        false,
    ));
    assert_eq!(updates, observer.updates.get());
}

#[test]
fn layer_mut_guard_notifies_once_per_change() {
    // Not a C++ test: `update_layer` stands in for
    // Layer::observer->onLayerChanged (see layer.rs).
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background"}]}"#);
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(Rc::<CountingObserver>::clone(&observer)));

    let before = Arc::clone(style.get_layer_list());
    style.update_layer("bg", |layer| layer.get_id().len());
    assert_eq!(0, observer.updates.get());
    assert!(Arc::ptr_eq(&before, style.get_layer_list()));

    let json = serde_json::json!("white");
    let error = style.update_layer("bg", |layer| layer.set_property("background-color", &(&json).into(), None));
    assert!(error.unwrap().is_none());
    assert_eq!(1, observer.updates.get());
    assert!(!Arc::ptr_eq(&before, style.get_layer_list()));
}

#[test]
fn layer_changes_leave_earlier_snapshots_unchanged() {
    // Not a C++ test: the collection changes its impls in place unless a
    // snapshot shares them.
    let mut style = stub_style();
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(Rc::<CountingObserver>::clone(&observer)));
    style.load_json(
        r#"{"version":8,"sources":{},"layers":[{"id":"a","type":"background"},{"id":"b","type":"background"}]}"#,
    );
    // A parse doesn't notify: the map publishes the parsed style once.
    assert_eq!(0, observer.updates.get());
    let ids = |layers: &crate::LayerList| layers.iter().map(|layer| layer.get_id().to_owned()).collect::<Vec<_>>();
    let loaded = Arc::clone(style.get_layer_list());
    assert_eq!(["a", "b"], *ids(&loaded));

    style.add_layer(new_layer("line", "c", "source"), Some("b")).unwrap();
    let error = style.add_layer(new_layer("line", "a", "source"), Some("b")).err().unwrap();
    assert_eq!(mln_util::ErrorKind::AlreadyExists, error.kind);
    assert_eq!("Layer a already exists", error.message);
    assert_eq!(1, observer.updates.get());
    let added = Arc::clone(style.get_layer_list());
    assert_eq!(["a", "c", "b"], *ids(&added));

    assert!(style.remove_layer("a").is_some());
    assert_eq!(["c", "b"], *ids(style.get_layer_list()));
    assert_eq!(["a", "b"], *ids(&loaded));
    assert_eq!(["a", "c", "b"], *ids(&added));
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
    style.set_global_state_property("showLabels", &Value::Bool(false));
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);

    // Properties that are not defined in the style can also be set.
    style.set_global_state_property("custom", &Value::Double(42.0));
    assert_eq!(Value::Double(42.0), style.get_global_state()["custom"]);

    // A null value resets the property to its default.
    style.set_global_state_property("showLabels", &Value::Null);
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);

    // A null value on a property without default resets to null.
    style.set_global_state_property("custom", &Value::Null);
    assert_eq!(Value::Null, style.get_global_state()["custom"]);

    // Loading a new style resets the global state.
    style.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    assert!(style.get_global_state().is_empty());

    // As in GL JS, setting an absent property to null creates an observable
    // null-valued entry even though both forms evaluate to null in expressions.
    let before_null = style.get_global_state_shared();
    style.set_global_state_property("neverSet", &Value::Null);
    assert!(!Arc::ptr_eq(&before_null, &style.get_global_state_shared()));
    let state = style.get_global_state();
    assert_eq!(1, state.len());
    assert_eq!(Value::Null, state["neverSet"]);

    // Repeating the same null assignment remains a no-op.
    let after_null = style.get_global_state_shared();
    style.set_global_state_property("neverSet", &Value::Null);
    assert!(Arc::ptr_eq(&after_null, &style.get_global_state_shared()));
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
    let before = style.get_global_state_shared();
    style.set_global_state_property("minSpeed", &Value::Double(50.0));
    assert!(Arc::ptr_eq(&before, &style.get_global_state_shared()));

    style.set_global_state_property("minSpeed", &Value::Double(51.0));
    assert!(!Arc::ptr_eq(&before, &style.get_global_state_shared()));
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

    style.set_global_state_property("showBackground", &Value::Bool(false));
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());

    style.set_global_state_property("showBackground", &Value::Null);
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    // Setting a constant visibility replaces the expression: further state
    // changes no longer apply.
    style.update_layer("background", |layer| layer.set_visibility(VisibilityType::None)).unwrap();
    style.set_global_state_property("showBackground", &Value::Bool(true));
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
    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showBackground"]], "visible", "none"]"#);
    let error = style
        .update_layer("background", |layer| layer.set_property("visibility", &Convertible::from(&json), None))
        .unwrap();
    assert!(error.is_none());
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());

    // Runtime global state (not only style defaults) applies as well.
    style.set_global_state_property("showBackground", &Value::Bool(true));
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showBackground"]], "none", "visible"]"#);
    let error = style
        .update_layer("background", |layer| layer.set_property("visibility", &Convertible::from(&json), None))
        .unwrap();
    assert!(error.is_none());
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());
    assert!(log.is_empty(), "{:?}", log.messages());
}

// gtest: Style.LayerAddedAfterGlobalStateUpdateUsesCurrentState
#[test]
fn layer_added_after_global_state_update_uses_current_state() {
    let mut style = stub_style();
    style.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    style.set_global_state_property("showLayer", &Value::Bool(false));

    let mut layer = new_layer("line", "line", "source");
    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showLayer"]], "visible", "none"]"#);
    assert!(layer.set_property("visibility", &Convertible::from(&json), None).is_none());
    assert_eq!(VisibilityType::Visible, layer.get_visibility());

    style.add_layer(layer, None).unwrap();
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
    let json = parse_json(r#"["case", [">", ["zoom"], 10], "visible", "none"]"#);
    let error = style
        .update_layer("background", |layer| layer.set_property("visibility", &Convertible::from(&json), None))
        .unwrap();
    assert!(error.is_some());
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
    assert_eq!(Color::blue(), *sky.properties.sky_color.value.as_constant());
    assert_float_eq(0.4, *sky.properties.sky_atmosphere_blend.value.as_constant());

    let mut replacement = Sky::new();
    assert!(
        replacement
            .set_property("fog-color", &Convertible::from(&serde_json::json!("red")), &Functions::with_builtins())
            .is_none()
    );
    style.set_sky(Some(replacement));
    let sky = style.get_sky().expect("sky");
    assert!(sky.properties.sky_color.value.is_undefined());
    assert!(sky.properties.sky_atmosphere_blend.value.is_undefined());
    assert_eq!(Color::red(), *sky.properties.sky_fog_color.value.as_constant());

    style.set_sky(None);
    assert!(style.get_sky().is_none());

    style.load_json(r#"{"sky":{"sky-color":"white"}}"#);
    assert!(style.get_sky().is_some());
    style.load_json(r"{}");
    assert!(style.get_sky().is_none());

    style.load_json(r#"{"sky":[]}"#);
    assert!(style.get_sky().is_none());
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

    assert_eq!(Color::blue(), *style.get_sky().expect("sky").properties.sky_color.value.as_constant());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant());
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);

    style.set_global_state_property("showLabels", &Value::Bool(false));
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
    assert_eq!(Color::blue(), *style.get_sky().expect("sky").properties.sky_color.value.as_constant());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant());

    style.set_sky(None);
    assert!(style.get_sky().is_none());
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
}

fn vector_source(id: &str) -> TileSource {
    TileSource::vector(id, Some("mptiler://tiles/contours".to_owned()), PartialTileset::default())
}

#[test]
fn source_records_keep_the_order_they_were_added_in() {
    let mut style = Style::new(StyleRegistry::with_builtins());
    for id in ["c", "b", "a"] {
        style.set_source_impl(id, Some(vector_source(id).base_impl()));
    }
    let ids = |style: &Style| style.get_sources().iter().map(|source| source.id.clone()).collect::<Vec<_>>();
    assert_eq!(["c", "b", "a"], *ids(&style));
    assert!(style.is_mutated());

    // A new record of a source replaces the old one in place.
    let before = Arc::clone(style.get_sources());
    let record = vector_source("b").base_impl();
    assert!(style.refresh_source_impl("b", Arc::clone(&record)));
    assert_eq!(["c", "b", "a"], *ids(&style));
    assert!(Arc::ptr_eq(&record, style.get_source("b").unwrap()));
    assert!(!Arc::ptr_eq(&before, style.get_sources()));
    // The same record changes nothing.
    let after = Arc::clone(style.get_sources());
    assert!(!style.refresh_source_impl("b", record));
    assert!(Arc::ptr_eq(&after, style.get_sources()));

    style.set_source_impl("b", None);
    assert_eq!(["c", "a"], *ids(&style));
    assert_eq!(["c", "b", "a"], *ids_of(&before));
}

fn ids_of(sources: &crate::SourceList) -> Vec<String> {
    sources.iter().map(|source| source.id.clone()).collect()
}
