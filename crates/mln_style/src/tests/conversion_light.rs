use super::util::parse_json;
use crate::color::Color;
use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::light::convert_light;
use crate::property::value::StyleValue as _;
use crate::types::Position;

// gtest: StyleConversion.Light
#[test]
fn light() {
    let mut error = Error::default();
    let parse_light = |src: &str, error: &mut Error| {
        let json = parse_json(src);
        convert_light(&Convertible::from(&json), &Functions::with_builtins(), error)
    };

    {
        let mut light = parse_light("{}", &mut error).expect("light");

        let color_value = serde_json::json!("blue");
        light.set_property("color", &Convertible::from(&color_value), &Functions::with_builtins());

        assert!(!light.properties.light_color.value.is_undefined());
        assert!(light.properties.light_color.value.is_constant());
        assert_eq!(*light.properties.light_color.value.as_constant().unwrap(), Color::blue());

        let intensity_value = serde_json::json!(0.5);
        light.set_property("intensity", &Convertible::from(&intensity_value), &Functions::with_builtins());
        assert!(!light.properties.light_intensity.value.is_undefined());
        assert!(light.properties.light_intensity.value.is_constant());
        assert_eq!(*light.properties.light_intensity.value.as_constant().unwrap(), 0.5);

        let position_value = serde_json::json!([1.0, 2.0, 3.0]);
        light.set_property("position", &Convertible::from(&position_value), &Functions::with_builtins());
        assert!(!light.properties.light_position.value.is_undefined());
        assert!(light.properties.light_position.value.is_constant());
        assert_eq!(*light.properties.light_position.value.as_constant().unwrap(), Position::new([1.0, 2.0, 3.0]));
    }

    {
        let light = parse_light(
            r#"{"color":{"stops":[[14,"blue"],[16,"red"]]},"intensity":0.3,"position":[3,90,90]}"#,
            &mut error,
        )
        .expect("light");

        assert!(light.properties.light_anchor.value.is_undefined());
        assert!(!light.properties.light_anchor.value.is_constant());
        assert!(!light.properties.light_anchor.value.is_expression());

        assert!(!light.properties.light_intensity.value.is_undefined());
        assert!(light.properties.light_intensity.value.is_constant());
        assert_eq!(*light.properties.light_intensity.value.as_constant().unwrap(), 0.3_f32);
        assert!(!light.properties.light_intensity.value.is_expression());

        assert!(!light.properties.light_color.value.is_undefined());
        assert!(!light.properties.light_color.value.is_constant());
        assert!(light.properties.light_color.value.is_expression());
        assert!(!light.properties.light_color.value.as_expression().unwrap().is_zoom_constant());

        assert!(!light.properties.light_position.value.is_undefined());
        assert!(light.properties.light_position.value.is_constant());
        assert_eq!(*light.properties.light_position.value.as_constant().unwrap(), Position::new([3.0, 90.0, 90.0]));
        assert!(!light.properties.light_position.value.is_expression());
    }

    {
        let light = parse_light(r#"{"color":"blue","intensity":0.3,"color-transition":{"duration":1000}}"#, &mut error)
            .expect("light");

        assert!(!light.properties.light_color.value.is_undefined());
        assert!(light.properties.light_color.value.is_constant());
        assert!(!light.properties.light_color.value.is_expression());
        assert_eq!(light.properties.light_color.options.duration, Some(std::time::Duration::from_millis(1000)));
        assert!(light.properties.light_color.options.delay.is_none());
    }

    {
        let light = parse_light(r#"{"intensity":false}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be a number", error.message);
    }

    {
        let light = parse_light(r#"{"intensity":{"stops":[[15,"red"],[17,"blue"]]}}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("Expected number but found string instead.", error.message);
    }

    {
        let light = parse_light(r#"{"color":5}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be a string", error.message);
    }

    {
        let light = parse_light(r#"{"position":[0,5]}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be an array of 3 numbers", error.message);
    }

    {
        let light = parse_light(r#"{"anchor":"something"}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be a valid enumeration value", error.message);
    }
}
