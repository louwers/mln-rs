// Ports src/mln/renderer/possibly_evaluated_property_value.hpp (a cut point of the C++ include cycle).

use mln_expression::expression::FeatureState;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::interpolate::Interpolate;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, ValueConverter};
use crate::property_expression::PropertyExpression;
use crate::renderer::cross_faded_property_evaluator::Faded;

/// `PossiblyEvaluatedPropertyValue<T>`: `variant<T, PropertyExpression<T>>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PossiblyEvaluatedPropertyValue<T> {
    Constant(T),
    Expression(PropertyExpression<T>),
}

impl<T: Default> Default for PossiblyEvaluatedPropertyValue<T> {
    fn default() -> Self {
        PossiblyEvaluatedPropertyValue::Constant(T::default())
    }
}

impl<T: Clone> PossiblyEvaluatedPropertyValue<T> {
    pub fn is_constant(&self) -> bool {
        matches!(self, PossiblyEvaluatedPropertyValue::Constant(_))
    }

    pub fn is_expression(&self) -> bool {
        matches!(self, PossiblyEvaluatedPropertyValue::Expression(_))
    }

    pub fn constant(&self) -> Option<T> {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(t) => Some(t.clone()),
            PossiblyEvaluatedPropertyValue::Expression(_) => None,
        }
    }

    pub fn constant_or(&self, t: &T) -> T {
        self.constant().unwrap_or_else(|| t.clone())
    }

    /// `evaluate(float zoom)`.
    pub fn evaluate_zoom(&self, zoom: f32) -> T
    where
        T: Default + ValueConverter,
    {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(constant) => constant.clone(),
            PossiblyEvaluatedPropertyValue::Expression(expression) => expression.evaluate_zoom(zoom),
        }
    }

    /// `evaluate(const Feature&, float zoom, T defaultValue)`.
    pub fn evaluate_feature(&self, feature: &dyn GeometryTileFeature, zoom: f32, default_value: T) -> T
    where
        T: ValueConverter,
    {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(constant) => constant.clone(),
            PossiblyEvaluatedPropertyValue::Expression(expression) => {
                expression.evaluate_zoom_feature(zoom, feature, default_value)
            }
        }
    }

    /// `evaluate(const Feature&, float zoom, const CanonicalTileID&, T defaultValue)`.
    pub fn evaluate_feature_canonical(
        &self,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        canonical: &CanonicalTileID,
        default_value: T,
    ) -> T
    where
        T: ValueConverter,
    {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(constant) => constant.clone(),
            PossiblyEvaluatedPropertyValue::Expression(expression) => {
                expression.evaluate_zoom_feature_canonical(zoom, feature, canonical, default_value)
            }
        }
    }

    /// `evaluate(const Feature&, float zoom, const FeatureState&, T defaultValue)`.
    pub fn evaluate_feature_state(
        &self,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        feature_state: &FeatureState,
        default_value: T,
    ) -> T
    where
        T: ValueConverter,
    {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(constant) => constant.clone(),
            PossiblyEvaluatedPropertyValue::Expression(expression) => {
                expression.evaluate_zoom_feature_state(zoom, feature, feature_state, default_value)
            }
        }
    }

    pub fn get_dependencies(&self) -> Dependency {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(_) => Dependency::NONE,
            PossiblyEvaluatedPropertyValue::Expression(expression) => expression.get_dependencies(),
        }
    }

    pub fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(_) => None,
            PossiblyEvaluatedPropertyValue::Expression(expression) => expression.get_global_state_refs(),
        }
    }
}

/// `PossiblyEvaluatedPropertyValue<Faded<T>>`, the C++ partial
/// specialization: `variant<Faded<T>, PropertyExpression<T>>` (the
/// expression yields a `T`, not a `Faded<T>`), for the cross-faded
/// data-driven properties (fill-pattern, line-pattern, fill-extrusion-pattern).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PossiblyEvaluatedFadedPropertyValue<T> {
    Constant(Faded<T>),
    Expression(PropertyExpression<T>),
}

impl<T: Default> Default for PossiblyEvaluatedFadedPropertyValue<T> {
    fn default() -> Self {
        PossiblyEvaluatedFadedPropertyValue::Constant(Faded::default())
    }
}

impl<T: Clone> PossiblyEvaluatedFadedPropertyValue<T> {
    pub fn is_constant(&self) -> bool {
        matches!(self, PossiblyEvaluatedFadedPropertyValue::Constant(_))
    }

    pub fn constant(&self) -> Option<Faded<T>> {
        match self {
            PossiblyEvaluatedFadedPropertyValue::Constant(t) => Some(t.clone()),
            PossiblyEvaluatedFadedPropertyValue::Expression(_) => None,
        }
    }

    pub fn constant_or(&self, t: &Faded<T>) -> Faded<T> {
        self.constant().unwrap_or_else(|| t.clone())
    }

    /// `evaluate(const Feature&, float zoom, availableImages, canonical, T defaultValue)`.
    pub fn evaluate(
        &self,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        available_images: &VecSet<String>,
        canonical: &CanonicalTileID,
        default_value: T,
    ) -> Faded<T>
    where
        T: ValueConverter,
    {
        match self {
            PossiblyEvaluatedFadedPropertyValue::Constant(constant) => constant.clone(),
            PossiblyEvaluatedFadedPropertyValue::Expression(expression) => {
                if !expression.is_zoom_constant() {
                    let min = expression.evaluate_zoom_feature_images_canonical(
                        zoom.floor(),
                        feature,
                        available_images,
                        canonical,
                        default_value.clone(),
                    );
                    let max = expression.evaluate_zoom_feature_images_canonical(
                        zoom.floor() + 1.0,
                        feature,
                        available_images,
                        canonical,
                        default_value,
                    );
                    Faded { from: min, to: max }
                } else {
                    let evaluated = expression.evaluate_feature_images(feature, available_images, default_value);
                    Faded { from: evaluated.clone(), to: evaluated }
                }
            }
        }
    }

    pub fn get_dependencies(&self) -> Dependency {
        match self {
            PossiblyEvaluatedFadedPropertyValue::Constant(_) => Dependency::NONE,
            PossiblyEvaluatedFadedPropertyValue::Expression(expression) => expression.get_dependencies(),
        }
    }

    pub fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        match self {
            PossiblyEvaluatedFadedPropertyValue::Constant(_) => None,
            PossiblyEvaluatedFadedPropertyValue::Expression(expression) => expression.get_global_state_refs(),
        }
    }
}

/// `Interpolator<PossiblyEvaluatedPropertyValue<Faded<T>>>`: both overloads
/// return `a` (constant `Faded` values are `Uninterpolated`, and a non-constant
/// pair returns `a` as for every possibly-evaluated value).
impl<T: Clone> Interpolate for PossiblyEvaluatedFadedPropertyValue<T> {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        self.clone()
    }
}

/// `Interpolator<PossiblyEvaluatedPropertyValue<T>>` (both overloads).
impl<T: Clone + Interpolate> Interpolate for PossiblyEvaluatedPropertyValue<T> {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        match (self, to) {
            (PossiblyEvaluatedPropertyValue::Constant(a), PossiblyEvaluatedPropertyValue::Constant(b)) => {
                PossiblyEvaluatedPropertyValue::Constant(a.interpolate(b, t))
            }
            _ => self.clone(),
        }
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        match (self, to) {
            (PossiblyEvaluatedPropertyValue::Constant(a), PossiblyEvaluatedPropertyValue::Constant(b)) => {
                PossiblyEvaluatedPropertyValue::Constant(a.interpolate_f32(b, t))
            }
            _ => self.clone(),
        }
    }
}
