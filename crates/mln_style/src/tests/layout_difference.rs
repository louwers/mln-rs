// What changes of a layer lay its tiles out again: the rule of
// `layer_type::layout_difference`, per layer type.

use mln_conversion::Convertible;

use super::util::{new_layer, parse_json};
use crate::layer::Layer;

/// Whether setting each `(name, json)` of `changes` on a copy of `layer`
/// lays its tiles out again.
fn relayouts(mut layer: Layer, changes: &[(&str, &str)]) -> bool {
    let before = std::sync::Arc::clone(layer.style());
    for (name, json) in changes {
        let value = parse_json(json);
        let error = layer.set_property(name, &Convertible::from(&value), None);
        assert!(error.is_none(), "{name}: {error:?}");
    }
    layer.style().layout_difference(&*before)
}

const FILTER: (&str, &str) = ("filter", r#"["==", "class", "road"]"#);
const HIDDEN: (&str, &str) = ("visibility", r#""none""#);

#[test]
fn geometry_types_relayout_on_the_filter_and_visibility() {
    for type_ in ["circle", "fill", "fill-extrusion", "heatmap", "line", "symbol"] {
        assert!(relayouts(new_layer(type_, "l", "s"), &[FILTER]), "{type_}");
        assert!(relayouts(new_layer(type_, "l", "s"), &[HIDDEN]), "{type_}");
    }
}

#[test]
fn types_without_geometry_never_relayout() {
    let layers = [
        ("background", ("background-opacity", "0.5")),
        ("color-relief", ("color-relief-opacity", "0.5")),
        ("hillshade", ("hillshade-exaggeration", "0.1")),
        ("raster", ("raster-opacity", "0.5")),
    ];
    for (type_, change) in layers {
        assert!(!relayouts(new_layer(type_, "l", "s"), &[HIDDEN, change]), "{type_}");
    }
}

#[test]
fn layout_properties_relayout() {
    let changes = [
        ("circle", ("circle-sort-key", "1")),
        ("fill", ("fill-sort-key", "1")),
        ("fill-extrusion", ("fill-extrusion-rounded-corner-distance", "2")),
        ("line", ("line-cap", r#""round""#)),
        ("symbol", ("text-size", "20")),
    ];
    for (type_, change) in changes {
        assert!(relayouts(new_layer(type_, "l", "s"), &[change]), "{type_}");
    }
}

#[test]
fn data_driven_paint_values_relayout_and_constants_dont() {
    let changes = [
        ("circle", "circle-color"),
        ("fill", "fill-color"),
        ("fill-extrusion", "fill-extrusion-color"),
        ("heatmap", "heatmap-weight"),
        ("line", "line-color"),
        ("symbol", "text-color"),
    ];
    for (type_, name) in changes {
        let constant = if name.ends_with("-color") { r##""#ff0000""## } else { "2" };
        assert!(!relayouts(new_layer(type_, "l", "s"), &[(name, constant)]), "{type_} {name}");
        let data_driven = if name.ends_with("-color") { r#"["get", "color"]"# } else { r#"["get", "weight"]"# };
        assert!(relayouts(new_layer(type_, "l", "s"), &[(name, data_driven)]), "{type_} {name}");
    }
}

#[test]
fn a_constant_fill_pattern_relayouts_and_other_patterns_dont() {
    assert!(relayouts(new_layer("fill", "l", "s"), &[("fill-pattern", r#""dots""#)]));
    assert!(!relayouts(new_layer("line", "l", "s"), &[("line-pattern", r#""dots""#)]));
    assert!(!relayouts(new_layer("fill-extrusion", "l", "s"), &[("fill-extrusion-pattern", r#""dots""#)]));
}

/// A symbol layer with the `text-field` `text_field` and a green
/// `text-color`.
fn green_symbol(text_field: &str) -> Layer {
    let mut layer = new_layer("symbol", "l", "s");
    for (name, json) in [("text-field", text_field), ("text-color", r#""green""#)] {
        let value = parse_json(json);
        assert!(layer.set_property(name, &Convertible::from(&value), None).is_none());
    }
    layer
}

#[test]
fn a_constant_text_color_relayouts_while_a_section_overrides_it() {
    let overriding = green_symbol(r#"["format", "a", {"text-color": "blue"}]"#);
    assert!(relayouts(overriding, &[("text-color", r##""#ff0000""##)]));
    let plain = green_symbol(r#""a""#);
    assert!(!relayouts(plain, &[("text-color", r##""#ff0000""##)]));
}
