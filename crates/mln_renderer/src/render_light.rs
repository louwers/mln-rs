// Ports src/mln/renderer/render_light.hpp and src/mln/renderer/render_light.cpp.

use mln_style::light::{self, LightProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::immutable::Immutable;

/// `TransitioningLight = style::LightProperties::Unevaluated`.
pub type TransitioningLight = LightProperties::Unevaluated;
/// `EvaluatedLight = style::LightProperties::PossiblyEvaluated`.
pub type EvaluatedLight = LightProperties::PossiblyEvaluated;

/// `RenderLight`.
pub struct RenderLight {
    pub impl_: Immutable<light::Impl>,
    transitioning: TransitioningLight,
    evaluated: EvaluatedLight,
}

impl RenderLight {
    /// `RenderLight(Immutable<style::Light::Impl>)`.
    #[must_use]
    pub fn new(impl_: Immutable<light::Impl>) -> Self {
        let transitioning = impl_.properties.untransitioned();
        RenderLight { impl_, transitioning, evaluated: EvaluatedLight::default() }
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

    pub fn get_evaluated(&self) -> &EvaluatedLight {
        &self.evaluated
    }
}
