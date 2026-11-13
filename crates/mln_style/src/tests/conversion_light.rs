// Ports test/style/conversion/light.test.cpp.

use mln_conversion::Convertible;
use mln_style_types::Position;
use mln_util::Color;

use super::util::parse_json;
use crate::conversion::{Converter, Error};
use crate::light::Light;

// gtest: StyleConversion.Light
#[test]
fn light() {
    let mut error = Error::default();
    let parse_light = |src: &str, error: &mut Error| {
        let json = parse_json(src);
        <Light as Converter>::convert(&Convertible::from(&json), error)
    };

    {
        let mut light = parse_light("{}", &mut error).expect("light");

        let color_value = serde_json::json!("blue");
        light.set_property("color", &Convertible::from(&color_value));

        assert!(!light.get_color().is_undefined());
        assert!(light.get_color().is_constant());
        assert_eq!(*light.get_color().as_constant(), Color::blue());

        let intensity_value = serde_json::json!(0.5);
        light.set_property("intensity", &Convertible::from(&intensity_value));
        assert!(!light.get_intensity().is_undefined());
        assert!(light.get_intensity().is_constant());
        assert_eq!(*light.get_intensity().as_constant(), 0.5);

        let position_value = serde_json::json!([1.0, 2.0, 3.0]);
        light.set_property("position", &Convertible::from(&position_value));
        assert!(!light.get_position().is_undefined());
        assert!(light.get_position().is_constant());
        assert_eq!(*light.get_position().as_constant(), Position::new([1.0, 2.0, 3.0]));
    }

    {
        let light = parse_light(
            r#"{"color":{"stops":[[14,"blue"],[16,"red"]]},"intensity":0.3,"position":[3,90,90]}"#,
            &mut error,
        )
        .expect("light");

        assert!(light.get_anchor().is_undefined());
        assert!(!light.get_anchor().is_constant());
        assert!(!light.get_anchor().is_expression());

        assert!(!light.get_intensity().is_undefined());
        assert!(light.get_intensity().is_constant());
        assert_eq!(*light.get_intensity().as_constant(), 0.3_f32);
        assert!(!light.get_intensity().is_expression());

        assert!(!light.get_color().is_undefined());
        assert!(!light.get_color().is_constant());
        assert!(light.get_color().is_expression());
        assert!(!light.get_color().as_expression().is_zoom_constant());

        assert!(!light.get_position().is_undefined());
        assert!(light.get_position().is_constant());
        assert_eq!(*light.get_position().as_constant(), Position::new([3.0, 90.0, 90.0]));
        assert!(!light.get_position().is_expression());
    }

    {
        let light = parse_light(r#"{"color":"blue","intensity":0.3,"color-transition":{"duration":1000}}"#, &mut error)
            .expect("light");

        assert!(!light.get_color().is_undefined());
        assert!(light.get_color().is_constant());
        assert!(!light.get_color().is_expression());
        assert_eq!(light.get_color_transition().duration, Some(std::time::Duration::from_millis(1000)));
        assert!(light.get_color_transition().delay.is_none());
    }

    {
        let light = parse_light(r#"{"intensity":false}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be a number", error.message);
    }

    {
        let light = parse_light(r#"{"intensity":{"stops":[[15,"red"],[17,"blue"]]}}"#, &mut error);

        assert!(light.is_none());
        assert_eq!("value must be a number", error.message);
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
