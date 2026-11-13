use mln_style_types::{LightAnchorType, Position};
use mln_util::Color;

use super::{Converter, Convertible, Error, convert, convert_property_value};
use crate::light::Light;
use crate::transition_options::TransitionOptions;

impl Converter for Light {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Light> {
        if !value.is_object() {
            error.message = "light must be an object".to_owned();
            return None;
        }

        let mut light = Light::new();

        if let Some(anchor) = value.object_member("anchor") {
            let converted_anchor = convert_property_value::<LightAnchorType>(&anchor, error, false, false)?;
            light.set_anchor(converted_anchor);
        }

        if let Some(anchor_transition) = value.object_member("anchor-transition") {
            let transition = convert::<TransitionOptions>(&anchor_transition, error)?;
            light.set_anchor_transition(&transition);
        }

        if let Some(color) = value.object_member("color") {
            let converted_color = convert_property_value::<Color>(&color, error, false, false)?;
            light.set_color(converted_color);
        }

        if let Some(color_transition) = value.object_member("color-transition") {
            let transition = convert::<TransitionOptions>(&color_transition, error)?;
            light.set_color_transition(&transition);
        }

        if let Some(position) = value.object_member("position") {
            let converted_position = convert_property_value::<Position>(&position, error, false, false)?;
            light.set_position(converted_position);
        }

        if let Some(position_transition) = value.object_member("position-transition") {
            let transition = convert::<TransitionOptions>(&position_transition, error)?;
            light.set_position_transition(&transition);
        }

        if let Some(intensity) = value.object_member("intensity") {
            let converted_intensity = convert_property_value::<f32>(&intensity, error, false, false)?;
            light.set_intensity(converted_intensity);
        }

        if let Some(intensity_transition) = value.object_member("intensity-transition") {
            let transition = convert::<TransitionOptions>(&intensity_transition, error)?;
            light.set_intensity_transition(&transition);
        }

        Some(light)
    }
}
