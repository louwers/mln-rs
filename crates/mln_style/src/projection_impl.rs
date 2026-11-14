use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::expression::EvaluationContext;
use crate::projection::Projection;
use crate::property_value::PropertyValue;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub(crate) type_: PropertyValue<ProjectionDefinition>,
}

impl Impl {
    /// How finely tiles are subdivided for this projection, for the life of
    /// the style: anything that can render a globe at some zoom gets the
    /// globe granularity, so the Mercator hand-off at high zoom reuses the
    /// same tiles.
    #[must_use]
    pub fn get_subdivision_granularity(&self) -> SubdivisionGranularitySetting {
        if self.type_.is_undefined()
            || (self.type_.is_constant()
                && *self.type_.as_constant() == ProjectionDefinition::new(ProjectionType::Mercator))
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
