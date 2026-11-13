use std::sync::Arc;

use mln_style::sky::{self, SkyProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};

pub(crate) type TransitioningSky = SkyProperties::Unevaluated;
pub(crate) type EvaluatedSky = SkyProperties::PossiblyEvaluated;

pub(crate) struct RenderSky {
    pub(crate) impl_: Arc<sky::Impl>,
    transitioning: TransitioningSky,
    evaluated: EvaluatedSky,
}

impl RenderSky {
    #[must_use]
    pub(crate) fn new(impl_: Arc<sky::Impl>) -> Self {
        let transitioning = impl_.properties.untransitioned();
        RenderSky { impl_, transitioning, evaluated: EvaluatedSky::default() }
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

    pub(crate) fn get_evaluated(&self) -> &EvaluatedSky {
        &self.evaluated
    }
}
