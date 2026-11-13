// Ports test/style/style.test.cpp.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use feature::Value;
use mln_conversion::Convertible;
use mln_style_types::{ProjectionDefinition, VisibilityType};
use mln_util::chrono::milliseconds;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, EventSeverity};
use mln_util::size::Size;
use mln_util::{Color, LatLng};

use super::util::{FixtureLog, parse_json, read_file};
use crate::image;
use crate::layer::Layer;
use crate::layers::line_layer::{Impl as LineLayerImpl, LineLayer};
use crate::layers::line_layer_properties::LineColor;
use crate::observer::{Observer, SourceObserver};
use crate::projection::Projection;
use crate::property_value::PropertyValue;
use crate::sky::Sky;
use crate::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use crate::sources::tile_source::{UrlOrTileset, VectorSource};
use crate::style::{Impl, Style};
use crate::transition_options::TransitionOptions;

/// `Style::Impl style{fileSource, 1.0, threadPool}` with
/// `auto fileSource = std::make_shared<StubFileSource>();`, as the C++ style
/// tests construct it.
fn stub_style() -> Impl {
    let file_source =
        mln_storage::stub_file_source::StubFileSource::new(mln_storage::stub_file_source::ResponseType::Asynchronous);
    Impl::with_file_source(
        Some(Arc::new(file_source)),
        1.0,
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
    )
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
    assert_eq!(milliseconds(300), style.get_transition_options().duration.unwrap());
    assert_eq!(None, style.get_transition_options().delay);

    style.load_json(r#"{"transition": { "duration": 500, "delay": 50 }}"#);
    assert_eq!(milliseconds(500), style.get_transition_options().duration.unwrap());
    assert_eq!(milliseconds(50), style.get_transition_options().delay.unwrap());

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
    style.add_layer(Box::new(LineLayer::new("line", "source")), None).unwrap();
    let before = style.get_layer_impls();

    {
        let mut layer = style.get_layer_mut("line").unwrap();
        let line = layer.downcast_mut::<LineLayer>().unwrap();
        line.set_line_color_transition(&TransitionOptions::new(Some(milliseconds(123)), Some(milliseconds(45)), true));
    }
    let after = style.get_layer_impls();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(1, after.len());
    assert!(Arc::ptr_eq(&style.get_layer("line").unwrap().base_impl(), &after[0]));
    let impl_ = after[0].downcast_ref::<LineLayerImpl>().unwrap();
    assert_eq!(Some(milliseconds(123)), impl_.paint.get::<LineColor>().options.duration);
    assert_eq!(Some(milliseconds(45)), impl_.paint.get::<LineColor>().options.delay);
}

#[derive(Default)]
struct CountingObserver {
    updates: Cell<usize>,
}

impl SourceObserver for CountingObserver {}
impl Observer for CountingObserver {
    fn on_update(&self) {
        self.updates.set(self.updates.get() + 1);
    }
}

// gtest: Style.SourceCollectionPublishesUpdates
#[test]
fn source_collection_publishes_updates() {
    // The C++ observer takes the style's sources inside onUpdate; this counts the calls and reads
    // the sources after each (see transition_options_publish_updates).
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    style.add_layer(Box::new(LineLayer::new("line", "late")), None).unwrap();
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(observer.clone()));

    style.add_source(Box::new(GeoJSONSource::new("late", GeoJSONOptions::default_options()))).unwrap();
    assert_eq!(1, observer.updates.get());
    assert_eq!(1, style.get_source_impls().len());

    style.remove_layer("line");
    let updates = observer.updates.get();
    assert!(style.remove_source("late").is_some());
    assert_eq!(updates + 1, observer.updates.get());
    assert!(style.get_source_impls().is_empty());
}

// gtest: Style.ImageRemovalPublishesUpdates
#[test]
fn image_removal_publishes_updates() {
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    style.add_image(make_image("image"));
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(observer.clone()));
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
    style.set_observer(Some(observer.clone()));

    style.set_transition_options(&TransitionOptions::new(Some(milliseconds(123)), Some(milliseconds(45)), true));
    assert_eq!(Some(milliseconds(123)), style.get_transition_options().duration);
    assert_eq!(Some(milliseconds(45)), style.get_transition_options().delay);
    assert_eq!(1, observer.updates.get());
    style.set_transition_options(&TransitionOptions::new(Some(milliseconds(123)), Some(milliseconds(45)), false));
    assert!(!style.get_transition_options().enable_placement_transitions);
    assert_eq!(2, observer.updates.get());

    let updates = observer.updates.get();
    style.set_transition_options(&TransitionOptions::new(Some(milliseconds(123)), Some(milliseconds(45)), false));
    assert_eq!(updates, observer.updates.get());
}

#[test]
fn layer_mut_guard_notifies_once_per_change() {
    // Not a C++ test: the LayerRefMut guard stands in for
    // Layer::observer->onLayerChanged (see layer.rs).
    let mut style = stub_style();
    style.load_json(r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background"}]}"#);
    let observer = Rc::new(CountingObserver::default());
    style.set_observer(Some(observer.clone()));

    let before = style.get_layer_impls();
    {
        let layer = style.get_layer_mut("bg").unwrap();
        let _ = layer.get_id();
    }
    assert_eq!(0, observer.updates.get());
    assert!(Arc::ptr_eq(&before, &style.get_layer_impls()));

    {
        let json = serde_json::json!("white");
        let mut layer = style.get_layer_mut("bg").unwrap();
        assert!(layer.set_property("background-color", &(&json).into(), None).is_none());
    }
    assert_eq!(1, observer.updates.get());
    assert!(!Arc::ptr_eq(&before, &style.get_layer_impls()));
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
    style.get_layer_mut("background").unwrap().set_visibility(VisibilityType::None);
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
    let error = style.get_layer_mut("background").unwrap().set_property("visibility", &Convertible::from(&json), None);
    assert!(error.is_none());
    assert_eq!(VisibilityType::None, style.get_layer("background").unwrap().get_visibility());

    // Runtime global state (not only style defaults) applies as well.
    style.set_global_state_property("showBackground", &Value::Bool(true));
    assert_eq!(VisibilityType::Visible, style.get_layer("background").unwrap().get_visibility());

    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showBackground"]], "none", "visible"]"#);
    let error = style.get_layer_mut("background").unwrap().set_property("visibility", &Convertible::from(&json), None);
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

    let mut layer = LineLayer::new("line", "source");
    let json = parse_json(r#"["case", ["to-boolean", ["global-state", "showLayer"]], "visible", "none"]"#);
    assert!(layer.set_property("visibility", &Convertible::from(&json), None).is_none());
    assert_eq!(VisibilityType::Visible, layer.get_visibility());

    let added_layer = style.add_layer(Box::new(layer), None).unwrap();
    assert_eq!(VisibilityType::None, added_layer.get_visibility());
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
    let error = style.get_layer_mut("background").unwrap().set_property("visibility", &Convertible::from(&json), None);
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
    assert!(style.get_sky().expect("sky").get_sky_color().is_undefined());

    style.load_json(r#"{"sky":{"sky-color":"blue","atmosphere-blend":0.4}}"#);
    let sky = style.get_sky().expect("sky");
    assert_eq!(Color::blue(), *sky.get_sky_color().as_constant());
    assert_float_eq(0.4, *sky.get_atmosphere_blend().as_constant());

    let mut replacement = Sky::new();
    replacement.set_fog_color(PropertyValue::Constant(Color::red()));
    style.set_sky(Some(replacement));
    let sky = style.get_sky().expect("sky");
    assert!(sky.get_sky_color().is_undefined());
    assert!(sky.get_atmosphere_blend().is_undefined());
    assert_eq!(Color::red(), *sky.get_fog_color().as_constant());

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

    assert_eq!(Color::blue(), *style.get_sky().expect("sky").get_sky_color().as_constant());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant());
    assert_eq!(Value::Bool(true), style.get_global_state()["showLabels"]);

    style.set_global_state_property("showLabels", &Value::Bool(false));
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
    assert_eq!(Color::blue(), *style.get_sky().expect("sky").get_sky_color().as_constant());
    assert_eq!(ProjectionDefinition::from_name("globe"), *style.get_projection().get_type().as_constant());

    style.set_sky(None);
    assert!(style.get_sky().is_none());
    assert_eq!(Value::Bool(false), style.get_global_state()["showLabels"]);
}

fn vector_source(id: &str) -> VectorSource {
    VectorSource::new(
        id,
        UrlOrTileset::Url("mptiler://tiles/contours".to_owned()),
        None,
        None,
        mln_util::tileset::VectorEncoding::Mapbox,
    )
}

// gtest: Style.DuplicateSource
#[test]
fn duplicate_source() {
    let mut style = stub_style();

    style.load_json(&read_file("test/fixtures/resources/style-unused-sources.json"));

    style.add_source(Box::new(vector_source("sourceId"))).unwrap();

    // C++ throws std::runtime_error.
    assert!(
        style.add_source(Box::new(vector_source("sourceId"))).is_err(),
        "Should not have been allowed to add a duplicate source id"
    );
}

// gtest: Style.RemoveSourceInUse
#[test]
fn remove_source_in_use() {
    let log = FixtureLog::new();

    let mut style = stub_style();

    style.load_json(&read_file("test/fixtures/resources/style-unused-sources.json"));
    style.add_source(Box::new(vector_source("sourceId"))).unwrap();
    style.add_layer(Box::new(LineLayer::new("layerId", "sourceId")), None).unwrap();

    // Should not remove the source
    let removed = style.remove_source("sourceId");
    assert!(removed.is_none());
    assert!(style.get_source("sourceId").is_some());

    assert_eq!(1, log.count(EventSeverity::Warning, Event::General, "Source 'sourceId' is in use, cannot remove"));
}

// gtest: Style.PublicLoadStateDoesNotWaitForResources
#[test]
fn public_load_state_does_not_wait_for_resources() {
    let file_source =
        mln_storage::stub_file_source::StubFileSource::new(mln_storage::stub_file_source::ResponseType::Manual);
    let mut style = Style::with_file_source(
        Some(Arc::new(file_source)),
        1.0,
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
    );

    assert!(!style.is_loaded());

    style.load_json(
        r#"{
        "version": 8,
        "sources": {
            "pending": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": []
    }"#,
    );
    assert!(style.is_loaded());

    style.load_url("http://some-url");
    assert!(!style.is_loaded());
}

// gtest: Style.SourceImplsOrder
#[test]
fn source_impls_order() {
    let mut style = stub_style();

    style.add_source(Box::new(vector_source("c"))).unwrap();
    style.add_source(Box::new(vector_source("b"))).unwrap();
    style.add_source(Box::new(vector_source("a"))).unwrap();

    let sources = style.get_sources();
    assert_eq!(3, sources.len());
    assert_eq!("c", sources[0].get_id());
    assert_eq!("b", sources[1].get_id());
    assert_eq!("a", sources[2].get_id());

    let source_impls = style.get_source_impls();
    assert_eq!(3, source_impls.len());
    assert_eq!("a", source_impls[0].id);
    assert_eq!("b", source_impls[1].id);
    assert_eq!("c", source_impls[2].id);
}

// Style::loadURL (added by glfw).

/// An observer that records `on_style_response` and style errors.
#[derive(Default)]
struct ResponseObserver {
    responses: Cell<usize>,
    errors: std::cell::RefCell<Vec<crate::StyleLoadError>>,
}

impl SourceObserver for ResponseObserver {}

impl Observer for ResponseObserver {
    fn on_style_response(&self) {
        self.responses.set(self.responses.get() + 1);
    }
    fn on_style_error(&self, error: crate::StyleLoadError) {
        self.errors.borrow_mut().push(error);
    }
}

fn stub_with_style_response(
    responder: impl Fn(&mln_storage::Resource) -> Option<mln_storage::Response> + 'static,
) -> (Impl, Rc<ResponseObserver>, mln_actor::RunLoop) {
    let run_loop = mln_actor::RunLoop::new();
    let file_source =
        mln_storage::stub_file_source::StubFileSource::new(mln_storage::stub_file_source::ResponseType::Asynchronous);
    file_source.set_response(mln_storage::Kind::Style, responder);
    let mut style = Impl::with_file_source(
        Some(Arc::new(file_source)),
        1.0,
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
    );
    let observer = Rc::new(ResponseObserver::default());
    style.set_observer(Some(observer.clone()));
    (style, observer, run_loop)
}

#[test]
fn load_url_parses_the_response() {
    let (mut style, observer, run_loop) = stub_with_style_response(|resource| {
        assert_eq!(resource.url, "http://some-url");
        let mut response = mln_storage::Response::new();
        response.data = Some(Arc::new(br#"{"version": 8, "name": "Remote", "sources": {}, "layers": []}"#.to_vec()));
        Some(response)
    });
    style.load_url("http://some-url");
    assert!(!style.is_loaded());
    assert_eq!("http://some-url", style.get_url());
    run_loop.run_once();
    assert_eq!(1, observer.responses.get());
    style.process_style_responses();
    assert_eq!("Remote", style.get_name());
    assert_eq!("http://some-url", style.get_url());
}

#[test]
fn load_url_error() {
    let (mut style, observer, run_loop) = stub_with_style_response(|_| {
        let mut response = mln_storage::Response::new();
        response.error = Some(Box::new(mln_storage::response::ResourceError::new(
            mln_storage::response::Reason::NotFound,
            "HTTP status code 404",
        )));
        Some(response)
    });
    style.load_url("http://some-url");
    run_loop.run_once();
    style.process_style_responses();
    assert_eq!(
        vec![crate::StyleLoadError::Load("loading style failed: HTTP status code 404".to_owned())],
        *observer.errors.borrow()
    );
    assert!(!style.is_loaded());
}

#[test]
fn load_url_answered_without_a_style_fails() {
    for (no_content, message) in [
        (true, "the style request returned no content"),
        (false, "the style request returned not modified, but no earlier response delivered the style"),
    ] {
        let (mut style, observer, run_loop) = stub_with_style_response(move |_| {
            let mut response = mln_storage::Response::new();
            response.no_content = no_content;
            response.not_modified = !no_content;
            Some(response)
        });
        style.load_url("http://some-url");
        run_loop.run_once();
        style.process_style_responses();
        assert_eq!(
            vec![crate::StyleLoadError::Load(format!("loading style failed: {message}"))],
            *observer.errors.borrow()
        );
        assert!(!style.is_loaded());
    }
}

#[test]
fn load_json_parse_error() {
    let (mut style, observer, _run_loop) = stub_with_style_response(|_| None);
    style.load_json("[]");
    assert_eq!(
        vec![crate::StyleLoadError::Parse("Failed to parse style: style must be an object".to_owned())],
        *observer.errors.borrow()
    );
    assert!(!style.is_loaded());
}

// gtest: Style.LoadJSONCancelsPendingLoadURL
#[test]
fn load_json_cancels_pending_load_url() {
    let file_source = Arc::new(mln_storage::stub_file_source::StubFileSource::new(
        mln_storage::stub_file_source::ResponseType::Manual,
    ));
    file_source.set_response(mln_storage::Kind::Style, |_| {
        let mut response = mln_storage::Response::new();
        response.data = Some(Arc::new(read_file("test/fixtures/resources/style_vector.json").into_bytes()));
        Some(response)
    });
    let mut style = Impl::with_file_source(
        Some(file_source.clone()),
        1.0,
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
    );

    // Start loading a URL (this will be pending)
    style.load_url("http://some-url");

    // Before the URL request completes, load JSON directly
    style.load_json(
        r#"{
        "version": 8,
        "name": "Test Style",
        "sources": {},
        "layers": []
    }"#,
    );

    // The style should now be loaded with our JSON content
    assert_eq!("Test Style", style.get_name());
    assert_eq!("", style.get_url());
    assert!(style.get_json().contains("Test Style"));

    file_source.respond_to_all();
    style.process_style_responses();

    // The style should still show our JSON content, not the URL content
    assert_eq!("Test Style", style.get_name());
    assert_ne!("Streets", style.get_name());
}
