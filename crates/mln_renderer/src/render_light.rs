use std::sync::Arc;

use mln_style::light::{self, LightProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};

pub(crate) type TransitioningLight = LightProperties::Unevaluated;
pub(crate) type EvaluatedLight = LightProperties::PossiblyEvaluated;

pub(crate) struct RenderLight {
    pub(crate) impl_: Arc<light::Impl>,
    transitioning: TransitioningLight,
    evaluated: EvaluatedLight,
}

impl RenderLight {
    #[must_use]
    pub(crate) fn new(impl_: Arc<light::Impl>) -> Self {
        let transitioning = impl_.properties.untransitioned();
        RenderLight { impl_, transitioning, evaluated: EvaluatedLight::default() }
    }

    pub(crate) fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.transitioning);
        self.transitioning = self.impl_.properties.transitioned(parameters, prior);
    }

    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        self.evaluated = self.transitioning.evaluate(parameters);
    }

    pub(crate) fn has_transition(&self) -> bool {
        self.transitioning.has_transition()
    }

    pub(crate) fn get_evaluated(&self) -> &EvaluatedLight {
        &self.evaluated
    }
}
