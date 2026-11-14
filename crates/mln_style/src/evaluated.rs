//! A property's value for a frame: what is left of it once the zoom, the
//! global state and the time are known.

use mln_util::interpolate::Interpolate;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, EvaluationContext, ValueConverter};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;

/// A value that fades from one image or dash array to the next while the
/// zoom crosses an integer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Faded<T> {
    pub from: T,
    pub to: T,
}

/// How a property's value evaluates, from its style-spec property type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evaluation {
    /// To one value for every feature, or for a data-driven property, to an
    /// expression for each feature when the value reads the feature.
    Plain,
    /// As `Plain`, at the integer zoom below.
    IntegerZoom,
    /// To the values at two zooms to fade between: `line-dasharray` and
    /// `background-pattern`.
    CrossFaded,
    /// As `CrossFaded`, or to an expression for each feature when the value
    /// reads the feature or the images: the patterns.
    CrossFadedPerFeature,
}

/// A property's value for a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Evaluated<T> {
    /// The same for every feature.
    Constant(T),
    /// The same for every feature, fading between two values.
    CrossFaded(Faded<T>),
    /// Different for each feature: the expression evaluates for each, with
    /// `default` where it gives no value.
    PerFeature { expression: PropertyExpression<T>, default: T },
}

impl<T: Default> Default for Evaluated<T> {
    fn default() -> Self {
        Evaluated::Constant(T::default())
    }
}

impl<T: Clone + Default + ValueConverter> Evaluated<T> {
    /// Evaluates `value` for a frame, with `default` for an undefined value
    /// or an expression that gives no value.
    #[must_use]
    #[inline(never)]
    pub fn new(
        value: &PropertyValue<T>,
        evaluation: Evaluation,
        parameters: &PropertyEvaluationParameters,
        default: T,
    ) -> Self {
        let cross_fade = |min: T, mid: T, max: T| {
            if parameters.z > parameters.zoom_history.last_integer_zoom {
                Evaluated::CrossFaded(Faded { from: min, to: mid })
            } else {
                Evaluated::CrossFaded(Faded { from: max, to: mid })
            }
        };
        let expression = match value {
            PropertyValue::Undefined | PropertyValue::Constant(_) => {
                let value = match value {
                    PropertyValue::Constant(constant) => constant.clone(),
                    _ => default,
                };
                return match evaluation {
                    Evaluation::Plain | Evaluation::IntegerZoom => Evaluated::Constant(value),
                    Evaluation::CrossFaded | Evaluation::CrossFadedPerFeature => {
                        cross_fade(value.clone(), value.clone(), value)
                    }
                };
            }
            PropertyValue::Expression(expression) => expression,
        };
        let global_state = parameters.global_state.as_deref();
        let at = |z: f32, default: T| {
            expression.evaluate(&EvaluationContext::new(z).with_global_state(global_state), default)
        };
        let per_feature = |integer_zoom: bool, default: T| {
            let mut expression = expression.clone();
            expression.set_use_integer_zoom(integer_zoom);
            expression.capture_global_state(parameters.global_state.clone());
            Evaluated::PerFeature { expression, default }
        };
        let z = parameters.z;
        match evaluation {
            Evaluation::Plain | Evaluation::IntegerZoom => {
                let integer_zoom = evaluation == Evaluation::IntegerZoom;
                if expression.get_dependencies().intersects(Dependency::FEATURE | Dependency::IMAGE) {
                    per_feature(integer_zoom, default)
                } else {
                    Evaluated::Constant(at(if integer_zoom { z.floor() } else { z }, default))
                }
            }
            Evaluation::CrossFaded => {
                cross_fade(at(z - 1.0, T::default()), at(z, T::default()), at(z + 1.0, T::default()))
            }
            Evaluation::CrossFadedPerFeature => {
                if !expression.is_feature_constant() || !expression.is_runtime_constant() {
                    per_feature(false, default)
                } else {
                    let value = expression.evaluate(&EvaluationContext::new(z.floor()), T::default());
                    cross_fade(value.clone(), value.clone(), value)
                }
            }
        }
    }
}

impl<T> Evaluated<T> {
    #[must_use]
    pub fn is_constant(&self) -> bool {
        !matches!(self, Evaluated::PerFeature { .. })
    }

    /// The value for every feature: the constant, the faded-to value, or for
    /// a value that differs by feature, the default.
    #[must_use]
    pub fn uniform(&self) -> &T {
        match self {
            Evaluated::Constant(value) => value,
            Evaluated::CrossFaded(faded) => &faded.to,
            Evaluated::PerFeature { default, .. } => default,
        }
    }

    /// The value, if it is the same for every feature and doesn't fade.
    #[must_use]
    pub fn constant(&self) -> Option<T>
    where
        T: Clone,
    {
        match self {
            Evaluated::Constant(value) => Some(value.clone()),
            Evaluated::CrossFaded(_) | Evaluated::PerFeature { .. } => None,
        }
    }

    /// The value if it is the same for every feature and doesn't fade, or
    /// `default`.
    #[must_use]
    pub fn constant_or(&self, default: &T) -> T
    where
        T: Clone,
    {
        self.constant().unwrap_or_else(|| default.clone())
    }

    /// The values to fade between for every feature: the same value twice
    /// for a constant, or for a value that differs by feature, the default
    /// twice.
    #[must_use]
    pub fn faded(&self) -> Faded<&T> {
        match self {
            Evaluated::Constant(value) | Evaluated::PerFeature { default: value, .. } => {
                Faded { from: value, to: value }
            }
            Evaluated::CrossFaded(faded) => Faded { from: &faded.from, to: &faded.to },
        }
    }

    /// The expression of a value that differs by feature.
    #[must_use]
    pub fn expression(&self) -> Option<&PropertyExpression<T>> {
        match self {
            Evaluated::PerFeature { expression, .. } => Some(expression),
            Evaluated::Constant(_) | Evaluated::CrossFaded(_) => None,
        }
    }

    #[must_use]
    pub fn dependencies(&self) -> Dependency {
        self.expression().map_or(Dependency::NONE, |expression| expression.get_dependencies())
    }

    #[must_use]
    pub fn global_state_refs(&self) -> Option<&VecSet<String>> {
        self.expression().and_then(|expression| expression.get_global_state_refs())
    }
}

impl<T: Clone + ValueConverter> Evaluated<T> {
    /// The value for the feature in `context`.
    #[must_use]
    pub fn evaluate(&self, context: &EvaluationContext<'_>) -> T {
        match self {
            Evaluated::Constant(value) => value.clone(),
            Evaluated::CrossFaded(faded) => faded.to.clone(),
            Evaluated::PerFeature { expression, default } => expression.evaluate(context, default.clone()),
        }
    }

    /// The values to fade between for the feature in `context`: for an
    /// expression that reads the zoom, its values at the integer zooms
    /// around the context's.
    #[must_use]
    pub fn evaluate_faded(&self, context: &EvaluationContext<'_>) -> Faded<T> {
        match self {
            Evaluated::Constant(value) => Faded { from: value.clone(), to: value.clone() },
            Evaluated::CrossFaded(faded) => faded.clone(),
            Evaluated::PerFeature { expression, default } => {
                if expression.is_zoom_constant() {
                    let value = expression.evaluate(context, default.clone());
                    Faded { from: value.clone(), to: value }
                } else {
                    let zoom = context.zoom.unwrap_or_default().floor();
                    let from =
                        expression.evaluate(&EvaluationContext { zoom: Some(zoom), ..*context }, default.clone());
                    let to =
                        expression.evaluate(&EvaluationContext { zoom: Some(zoom + 1.0), ..*context }, default.clone());
                    Faded { from, to }
                }
            }
        }
    }
}

/// Interpolates two constants. Anything else, including a cross-fade, keeps
/// the value transitioned from until the transition ends.
impl<T: Clone + Interpolate> Interpolate for Evaluated<T> {
    fn interpolate(&self, to: &Self, t: f64) -> Self {
        match (self, to) {
            (Evaluated::Constant(a), Evaluated::Constant(b)) => Evaluated::Constant(a.interpolate(b, t)),
            _ => self.clone(),
        }
    }

    fn interpolate_f32(&self, to: &Self, t: f32) -> Self {
        match (self, to) {
            (Evaluated::Constant(a), Evaluated::Constant(b)) => Evaluated::Constant(a.interpolate_f32(b, t)),
            _ => self.clone(),
        }
    }
}
