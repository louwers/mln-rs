// Ports src/mln/renderer/render_sky.hpp and src/mln/renderer/render_sky.cpp.

use mln_style::sky::{self, SkyProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::immutable::Immutable;

/// `TransitioningSky = style::SkyProperties::Unevaluated`.
pub type TransitioningSky = SkyProperties::Unevaluated;
/// `EvaluatedSky = style::SkyProperties::PossiblyEvaluated`.
pub type EvaluatedSky = SkyProperties::PossiblyEvaluated;

/// `RenderSky`.
pub struct RenderSky {
    pub impl_: Immutable<sky::Impl>,
    transitioning: TransitioningSky,
    evaluated: EvaluatedSky,
}

impl RenderSky {
    /// `explicit RenderSky(Immutable<style::Sky::Impl>)`.
    #[must_use]
    pub fn new(impl_: Immutable<sky::Impl>) -> Self {
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
