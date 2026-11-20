use std::sync::Arc;

use mln_conversion::{Convertible, Error};
use mln_style_types::VisibilityType;

use super::util::{new_layer, parse_json};
use crate::conversion::convert_filter;
use crate::edit::Edit;
use crate::expression::Functions;
use crate::filter::Filter;
use crate::layer::Layer;
use crate::registry::StyleRegistry;
use crate::style::Style;

fn parse_filter(expression: &str) -> Filter {
    let mut error = Error::default();
    let json = parse_json(expression);
    convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error).expect("filter")
}

/// Sets the property `name` of `layer` from style JSON.
fn set(layer: &mut Layer, name: &str, json: &str) {
    let value = parse_json(json);
    let error = layer.set_property(name, &Convertible::from(&value), None);
    assert!(error.is_none(), "{name}: {error:?}");
}

const RED: &str = r##""#ff0000""##;

// gtest: Layer.Observer
#[test]
fn observer() {
    // TEST(Layer, Observer). C++ counts `StubLayerObserver::layerChanged`
    // calls; in the port a layer changes when its impl `Arc` is replaced
    // (see layer.rs), which is what a style version compares, so each
    // "notifies" check is "the impl pointer changed".
    let mut layer = new_layer("symbol", "symbol", "source");
    let notified = |layer: &mut Layer, f: &mut dyn FnMut(&mut Layer)| {
        let before = Arc::clone(layer.style());
        f(layer);
        !Arc::ptr_eq(&before, layer.style())
    };

    // Notifies observer on filter change.
    assert!(notified(&mut layer, &mut |l| l.set_filter(&parse_filter(r#"["==", "foo", "bar"]"#))));

    // Notifies observer on visibility change.
    assert!(notified(&mut layer, &mut |l| l.set_visibility(VisibilityType::None)));

    // Notifies observer on paint property change.
    assert!(notified(&mut layer, &mut |l| set(l, "text-color", RED)));

    // Notifies observer on layout property change.
    assert!(notified(&mut layer, &mut |l| set(l, "text-transform", r#""uppercase""#)));

    // Notifies observer on paint property transition change.
    assert!(notified(&mut layer, &mut |l| set(l, "text-color-transition", r#"{"duration": 300}"#)));

    // Does not notify observer on no-op visibility change.
    assert!(!notified(&mut layer, &mut |l| l.set_visibility(VisibilityType::None)));

    // Does not notify observer on no-op paint property change.
    assert!(!notified(&mut layer, &mut |l| set(l, "text-color", RED)));

    // Does not notify observer on no-op layout property change.
    assert!(!notified(&mut layer, &mut |l| set(l, "text-transform", r#""uppercase""#)));

    // Does not notify observer on no-op paint property transition change.
    assert!(!notified(&mut layer, &mut |l| set(l, "text-color-transition", r#"{"duration": 300}"#)));

    // Notifies observer on source-layer change.
    assert!(notified(&mut layer, &mut |l| l.set_source_layer("roads")));

    // Does not notify observer on no-op source-layer change.
    assert!(!notified(&mut layer, &mut |l| l.set_source_layer("roads")));

    // Notifies observer on source ID change.
    assert!(notified(&mut layer, &mut |l| l.set_source_id("other-source")));

    // Does not notify observer on no-op source ID change.
    assert!(!notified(&mut layer, &mut |l| l.set_source_id("other-source")));
}

fn fixture(path: &str) -> String {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// gtest: Layer.DuplicateLayer
#[test]
fn duplicate_layer() {
    // Setup style
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.load_json(&fixture("test/fixtures/resources/style-unused-sources.json"));

    // Add initial layer
    style
        .apply(Edit::AddLayer { layer: new_layer("symbol", "symbol", "unusedsource"), before: None })
        .expect("first add");

    // Try to add duplicate
    match style.apply(Edit::AddLayer { layer: new_layer("symbol", "symbol", "unusedsource"), before: None }) {
        Ok(()) => panic!("Should not have been allowed to add a duplicate layer id"),
        // Expected
        Err(e) => assert_eq!(mln_util::Error::already_exists("Layer symbol already exists"), e),
    }
}

// gtest: Layer.IncompatibleLayer
#[test]
fn incompatible_layer() {
    // Setup style
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.load_json(&fixture("test/fixtures/resources/style-unused-sources.json"));

    // Try to add duplicate
    match style.apply(Edit::AddLayer { layer: new_layer("shade", "shade", "unusedsource"), before: None }) {
        Ok(()) => panic!("Should not have been allowed to add an incompatible layer to the source"),
        // Expected
        Err(e) => assert_eq!(
            mln_util::Error::invalid_argument("Layer 'shade' is not compatible with source 'unusedsource'"),
            e
        ),
    }
}

// Layer.SymbolLayerOverrides needs mln_renderer's FormatSectionOverride
// (set_overrides): it is in crates/mln_renderer/tests/format_section_override_test.rs.
