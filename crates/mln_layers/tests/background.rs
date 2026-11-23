//! The background layer type's style half: its properties.

use mln_layers::Background;
use mln_style::PropertyValue;
use mln_style::conversion::Convertible;
use mln_style::expression::Image;
use mln_style::layer::Layer;
use mln_style::layer_type::LayerStyle;
use mln_util::Color;

mod support;
use support::builtin_registry;

// gtest: Layer.BackgroundProperties
#[test]
fn background_properties() {
    let mut layer = Layer::new(builtin_registry().style(), "background", "background", "").unwrap();
    assert_eq!("background", layer.info().name);

    let mut set = |name: &str, json: serde_json::Value| {
        let error = layer.set_property(name, &Convertible::from(&json), None);
        assert!(error.is_none(), "{name}: {error:?}");
    };
    set("background-color", serde_json::json!("#ff0000"));
    set("background-opacity", serde_json::json!(1));
    set("background-pattern", serde_json::json!("foo"));
    let paint = &LayerStyle::<Background>::recover_ref(&**layer.style()).unwrap().paint;
    assert_eq!(paint.background_color.value, PropertyValue::Constant(Color::new(1.0, 0.0, 0.0, 1.0)));
    assert_eq!(paint.background_opacity.value, PropertyValue::Constant(1.0));
    assert_eq!(paint.background_pattern.value, PropertyValue::Constant(Image::new("foo")));
}
