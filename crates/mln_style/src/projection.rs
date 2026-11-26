use std::sync::Arc;

use crate::conversion::to_value::make_style_property;
use crate::conversion::{Convertible, Error};
use crate::expression::{EvaluationContext, Functions};
use crate::property::convert::convert_property_value;
use crate::property::table::StyleProperty;
use crate::property::value::{PropertyValue, StyleValue as _};
use crate::subdivision_granularity::SubdivisionGranularitySetting;
use crate::types::{ProjectionDefinition, ProjectionType};

/// The style's root `projection` object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projection {
    type_: PropertyValue<ProjectionDefinition>,
}

impl Projection {
    #[must_use]
    pub fn new() -> Self {
        Projection::default()
    }

    #[cfg(test)]
    pub(crate) fn set_property(
        &mut self,
        name: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
    ) -> Option<Error> {
        if name != "type" {
            return Some(Error::new("projection doesn't support this property"));
        }
        let mut error = Error::default();
        let Some(type_) = convert_property_value::<ProjectionDefinition>(value, &mut error, false, false, functions)
        else {
            return Some(error);
        };
        self.set_type(type_);
        None
    }

    #[must_use]
    pub(crate) fn get_property(&self, name: &str) -> StyleProperty {
        if name == "type" {
            return make_style_property(&self.type_);
        }
        StyleProperty::default()
    }

    #[must_use]
    pub(crate) fn get_default_type() -> ProjectionDefinition {
        ProjectionDefinition::default()
    }

    #[must_use]
    pub fn get_type(&self) -> PropertyValue<ProjectionDefinition> {
        self.type_.clone()
    }

    pub fn set_type(&mut self, type_: PropertyValue<ProjectionDefinition>) {
        self.type_ = type_;
    }

    /// How finely tiles are subdivided for this projection, for the life of
    /// the style: anything that can render a globe at some zoom gets the
    /// globe granularity, so the Mercator hand-off at high zoom reuses the
    /// same tiles.
    #[must_use]
    pub fn get_subdivision_granularity(&self) -> SubdivisionGranularitySetting {
        if self.type_.is_undefined()
            || self.type_.as_constant().is_some_and(|t| *t == ProjectionDefinition::new(ProjectionType::Mercator))
        {
            return SubdivisionGranularitySetting::none();
        }
        SubdivisionGranularitySetting::globe()
    }

    /// The definition at a zoom level, with `globe` resolved; `mercator`
    /// when the style does not set one.
    #[must_use]
    pub fn evaluate(&self, zoom: f32) -> ProjectionDefinition {
        let definition = match &self.type_ {
            PropertyValue::Undefined => Projection::get_default_type(),
            PropertyValue::Constant(definition) => *definition,
            PropertyValue::Expression(expression) => {
                expression.evaluate(&EvaluationContext::new(zoom), Projection::get_default_type())
            }
        };
        definition.at_zoom(f64::from(zoom))
    }
}

/// Converts the style's `projection`, whose expressions parse against
/// `functions`.
pub fn convert_projection(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Projection> {
    if !value.is_object() {
        error.message = "projection must be an object".to_owned();
        return None;
    }

    let mut projection = Projection::new();

    if let Some(type_) = value.object_member("type") {
        let converted = convert_property_value::<ProjectionDefinition>(&type_, error, false, false, functions)?;
        projection.set_type(converted);
    }

    Some(projection)
}
