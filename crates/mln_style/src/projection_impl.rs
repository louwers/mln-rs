// Ports src/mln/style/projection_impl.hpp and the `Projection::Impl`
// members of src/mln/style/projection.cpp.

use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::projection::Projection;
use crate::property_value::PropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::PropertyEvaluator;

/// `Projection::Impl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub type_: PropertyValue<ProjectionDefinition>,
}

impl Impl {
    /// How finely tiles are subdivided for this projection, for the life of
    /// the style: anything that can render a globe at some zoom gets the
    /// globe granularity, so the Mercator hand-off at high zoom reuses the
    /// same tiles.
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
    pub fn evaluate(&self, zoom: f32) -> ProjectionDefinition {
        let parameters = PropertyEvaluationParameters::new(zoom);
        let definition = self.type_.evaluate(&PropertyEvaluator::new(&parameters, Projection::get_default_type()));
        definition.at_zoom(f64::from(zoom))
    }
}
