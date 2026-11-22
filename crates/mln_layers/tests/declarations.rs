//! A layer type declared outside the engine crates: its properties through
//! `mln_style::properties!` and a property table, and the binders of its
//! data-driven paint properties through `mln_render::paint_property_binders!`.
//! The declarations name only what they import, so the macros' expansions
//! need nothing else of this crate.

use mln_render::layer::{CrossFadedPaintPropertyBinder, PaintBinders as _, PaintPropertyBinder};
use mln_style::conversion::Convertible;
use mln_style::evaluated::Evaluated;
use mln_style::expression::{Functions, Image};
use mln_style::layer::Layer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType, style_entry};
use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
use mln_style::style_property::Kind;
use mln_style::{PropertyEvaluationParameters, PropertyValue, StyleRegistry};
use mln_util::Color;

mln_style::properties!(
    /// The paint properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the built-in layer types' property sets are")]
    pub mod DotPaintProperties: paint {
        dot_color: data_driven Color = Color::black(),
        dot_opacity: constant f32 = 1.0_f32,
        dot_pattern: cross_faded_data_driven Image = Image::default(),
        dot_radius: data_driven f32 = 5.0_f32,
    }
    derived {
        dot_floor_radius: integer_zoom f32 = 5.0_f32 => dot_radius,
    }
    use mln_style::expression::Image, mln_util::Color;
);

mln_style::properties!(
    /// The layout properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the built-in layer types' property sets are")]
    pub mod DotLayoutProperties: layout {
        dot_sort_key: data_driven f32 = 0.0_f32,
    }
);

static PROPERTIES: PropertyTable = PropertyTable::new(
    &[
        PropertyInfo::paint("dot-color").data_driven(),
        PropertyInfo::paint("dot-opacity"),
        PropertyInfo::paint("dot-pattern").data_driven().cross_faded(),
        PropertyInfo::paint("dot-radius").data_driven(),
        PropertyInfo::layout("dot-sort-key").data_driven(),
    ],
    &[0, 1, 2, 3, 4],
);

/// The `dot` layer type.
struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = DotLayoutProperties::Unevaluated;
    type PaintProperties = DotPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => visitor.paint(|s| &s.paint.dot_opacity, |s| &mut s.paint.dot_opacity),
            2 => visitor.paint(|s| &s.paint.dot_pattern, |s| &mut s.paint.dot_pattern),
            3 => visitor.paint(|s| &s.paint.dot_radius, |s| &mut s.paint.dot_radius),
            4 => visitor.layout(|s| &s.layout.dot_sort_key, |s| &mut s.layout.dot_sort_key),
            _ => return None,
        })
    }
}

mln_render::paint_property_binders!(
    /// The binders of a `dot` layer's data-driven paint properties.
    struct DotBinders for DotPaintProperties::PossiblyEvaluated {
        dot_color: PaintPropertyBinder<Color> => ["color"],
        dot_radius: PaintPropertyBinder<f32> => ["radius"],
        dot_floor_radius: PaintPropertyBinder<f32> => ["floor_radius"],
        dot_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
    }
);

/// A `dot` layer with the paint and layout properties `properties` set.
fn layer(properties: &[(&str, serde_json::Value)]) -> Layer {
    let registry = StyleRegistry::new(Functions::with_builtins(), [style_entry::<Dot>()], []);
    let mut layer = Layer::new(&registry, "dot", "dot", "source").unwrap();
    for (name, value) in properties {
        let error = layer.set_property(name, &Convertible::from(value), None);
        assert!(error.is_none(), "{name}: {error:?}");
    }
    layer
}

fn style(layer: &Layer) -> &LayerStyle<Dot> {
    LayerStyle::<Dot>::recover_ref(&**layer.style()).unwrap()
}

#[test]
fn properties_set_and_get_by_name() {
    let mut layer = layer(&[
        ("dot-color", serde_json::json!("#ff0000")),
        ("dot-opacity-transition", serde_json::json!({ "duration": 300 })),
        ("dot-sort-key", serde_json::json!(2)),
    ]);
    let style = style(&layer);
    assert_eq!(style.paint.dot_color.value, PropertyValue::Constant(Color::new(1.0, 0.0, 0.0, 1.0)));
    assert_eq!(style.layout.dot_sort_key, PropertyValue::Constant(2.0));
    assert_eq!(layer.get_property("dot-opacity-transition").get_kind(), Kind::Transition);
    assert_eq!(layer.get_property("dot-radius").get_kind(), Kind::Undefined);
    assert!(layer.set_property("dot-size", &Convertible::from(&serde_json::json!(1)), None).is_some());
}

#[test]
fn properties_evaluate_to_their_defaults_and_derived_values() {
    let layer = layer(&[("dot-radius", serde_json::json!(8))]);
    let evaluated = style(&layer).paint.untransitioned().evaluate(&PropertyEvaluationParameters::new(3.0));
    assert_eq!(*evaluated.dot_color.uniform(), DotPaintProperties::Defaults::dot_color());
    assert_eq!(*evaluated.dot_radius.uniform(), 8.0);
    assert_eq!(*evaluated.dot_floor_radius.uniform(), 8.0);
}

#[test]
fn binders_bind_the_data_driven_properties() {
    let layer = layer(&[
        ("dot-color", serde_json::json!(["feature-state", "color"])),
        ("dot-radius", serde_json::json!(["interpolate", ["linear"], ["zoom"], 0, ["get", "r"], 10, 10])),
    ]);
    let evaluated = style(&layer).paint.untransitioned().evaluate(&PropertyEvaluationParameters::new(3.0));
    assert!(matches!(evaluated.dot_color, Evaluated::PerFeature { .. }));

    let binders = DotBinders::new(&evaluated, 3.0);
    assert!(binders.depends_on_feature_state());
    assert_eq!(binders.dot_radius.interpolation_factor(3.5), 0.5);
    assert_eq!(binders.dot_floor_radius.interpolation_factor(3.5), 0.0);
    assert_eq!(binders.dot_pattern.interpolation_factor(3.5), (0.0, 0.0));
}
