use std::sync::Arc;

use mln_style::sky::{Sky, SkyProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};

pub(crate) type TransitioningSky = SkyProperties::Unevaluated;
pub(crate) type EvaluatedSky = SkyProperties::PossiblyEvaluated;

pub(crate) struct RenderSky {
    pub(crate) sky: Arc<Sky>,
    transitioning: TransitioningSky,
    evaluated: EvaluatedSky,
}

impl RenderSky {
    #[must_use]
    pub(crate) fn new(sky: Arc<Sky>) -> Self {
        let transitioning = sky.properties.untransitioned();
        RenderSky { sky, transitioning, evaluated: EvaluatedSky::default() }
    }

    pub(crate) fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.transitioning);
        self.transitioning = self.sky.properties.transitioned(parameters, prior);
    }

    pub(crate) fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        self.evaluated = self.transitioning.evaluate(parameters);
    }

    pub(crate) fn has_transition(&self) -> bool {
        self.transitioning.has_transition()
    }

    pub(crate) fn evaluated(&self) -> &EvaluatedSky {
        &self.evaluated
    }
}
