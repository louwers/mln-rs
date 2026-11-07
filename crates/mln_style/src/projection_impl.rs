// Ports src/mln/style/projection_impl.hpp and the `Projection::Impl`
// members of src/mln/style/projection.cpp.

use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::interpolate::interpolate;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::projection::Projection;
use crate::property_value::PropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::PropertyEvaluator;

/// `std::clamp(x, 0.0, 1.0)`: `x < lo ? lo : hi < x ? hi : x` (NaN and -0.0
/// pass through).
fn clamp_unit(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if 1.0 < x {
        1.0
    } else {
        x
    }
}

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

    /// The definition at a zoom level; `mercator` when the style does not
    /// set one.
    pub fn evaluate(&self, zoom: f32) -> ProjectionDefinition {
        let parameters = PropertyEvaluationParameters::new(zoom);
        let definition = self.type_.evaluate(&PropertyEvaluator::new(&parameters, Projection::get_default_type()));
        if definition.from != ProjectionType::Globe && definition.to != ProjectionType::Globe {
            return definition;
        }
        // `globe` is the vertical perspective up to the first zoom and
        // Mercator from the second, blended in between, at either end of a
        // definition: each end counts with its share of the globe at this
        // zoom.
        const GLOBE_TO_MERCATOR_START_ZOOM: f32 = 11.0;
        const GLOBE_TO_MERCATOR_END_ZOOM: f32 = 12.0;
        let globe_share = 1.0
            - clamp_unit(
                (zoom - GLOBE_TO_MERCATOR_START_ZOOM) as f64
                    / (GLOBE_TO_MERCATOR_END_ZOOM - GLOBE_TO_MERCATOR_START_ZOOM) as f64,
            );
        let share = |end: ProjectionType| match end {
            ProjectionType::Globe => globe_share,
            ProjectionType::Mercator => 0.0,
            ProjectionType::VerticalPerspective => 1.0,
        };
        let globe = if definition.from == definition.to {
            share(definition.from)
        } else {
            interpolate(&share(definition.from), &share(definition.to), clamp_unit(definition.transition))
        };
        if globe >= 1.0 {
            return ProjectionDefinition::new(ProjectionType::VerticalPerspective);
        }
        if globe <= 0.0 {
            return ProjectionDefinition::new(ProjectionType::Mercator);
        }
        ProjectionDefinition::between(ProjectionType::VerticalPerspective, ProjectionType::Mercator, 1.0 - globe)
    }
}
