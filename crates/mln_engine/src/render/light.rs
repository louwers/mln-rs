use std::sync::Arc;

use mln_style::light::{Light, LightProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};

pub(crate) type TransitioningLight = LightProperties::Unevaluated;
/// The style's light, evaluated for the frame.
pub type EvaluatedLight = LightProperties::PossiblyEvaluated;

pub(crate) struct RenderLight {
    pub(crate) light: Arc<Light>,
    transitioning: TransitioningLight,
    evaluated: EvaluatedLight,
}

impl RenderLight {
    #[must_use]
    pub(crate) fn new(light: Arc<Light>) -> Self {
        let transitioning = light.properties.untransitioned();
        RenderLight { light, transitioning, evaluated: EvaluatedLight::default() }
    }

    pub(crate) fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.transitioning);
        self.transitioning = self.light.properties.transitioned(parameters, prior);
    }

    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        self.evaluated = self.transitioning.evaluate(parameters);
    }

    pub(crate) fn has_transition(&self) -> bool {
        self.transitioning.has_transition()
    }

    pub(crate) fn evaluated(&self) -> &EvaluatedLight {
        &self.evaluated
    }
}
