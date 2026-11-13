use std::sync::Arc;

use mln_style::sky::{self, SkyProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};

pub type TransitioningSky = SkyProperties::Unevaluated;
pub type EvaluatedSky = SkyProperties::PossiblyEvaluated;

pub struct RenderSky {
    pub impl_: Arc<sky::Impl>,
    transitioning: TransitioningSky,
    evaluated: EvaluatedSky,
}

impl RenderSky {
    #[must_use]
    pub fn new(impl_: Arc<sky::Impl>) -> Self {
        let transitioning = impl_.properties.untransitioned();
        RenderSky { impl_, transitioning, evaluated: EvaluatedSky::default() }
    }

    pub fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.transitioning);
        self.transitioning = self.impl_.properties.transitioned(parameters, prior);
    }

    pub fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        self.evaluated = self.transitioning.evaluate(parameters);
    }

    pub fn has_transition(&self) -> bool {
        self.transitioning.has_transition()
    }

    pub fn get_evaluated(&self) -> &EvaluatedSky {
        &self.evaluated
    }
}
