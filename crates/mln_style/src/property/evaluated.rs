//! A property's value for a frame: what is left of it once the zoom, the
//! global state and the time are known.

use mln_vec_map::VecSet;

use crate::expression::{Dependency, EvaluationContext, ValueConverter};
use crate::interpolate::Interpolate;
use crate::property::evaluation::PropertyEvaluationParameters;
use crate::property::expression::PropertyExpression;
use crate::property::value::PropertyValue;

/// A value that fades from one image or dash array to the next while the
/// zoom crosses an integer.
#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct Faded<T> {
    pub from: T,
    pub to: T,
}

/// How a property's value evaluates for a frame, with a default for an
/// undefined value or an expression that gives no value: one of
/// [`Evaluated::plain`], [`Evaluated::integer_zoom`],
/// [`Evaluated::cross_faded`] and [`Evaluated::cross_faded_per_feature`].
///
/// A function rather than an enum, so that only the value types that cross-fade
/// compile the cross-fade code.
pub type Evaluation<T> = fn(&PropertyValue<T>, &PropertyEvaluationParameters, T) -> Evaluated<T>;

/// A property's value for a frame.
#[derive(Clone, Debug, PartialEq, mln_wire::Wire)]
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
    /// To one value for every feature, or for a data-driven property, to an
    /// expression for each feature when the value reads the feature.
    pub fn plain(value: &PropertyValue<T>, parameters: &PropertyEvaluationParameters, default: T) -> Self {
        Self::uniform_or_per_feature(value, parameters, default, false)
    }

    /// As [`Evaluated::plain`], at the integer zoom below.
    pub fn integer_zoom(value: &PropertyValue<T>, parameters: &PropertyEvaluationParameters, default: T) -> Self {
        Self::uniform_or_per_feature(value, parameters, default, true)
    }

    #[inline(never)]
    fn uniform_or_per_feature(
        value: &PropertyValue<T>,
        parameters: &PropertyEvaluationParameters,
        default: T,
        integer_zoom: bool,
    ) -> Self {
        let expression = match value {
            PropertyValue::Undefined => return Evaluated::Constant(default),
            PropertyValue::Constant(constant) => return Evaluated::Constant(constant.clone()),
            PropertyValue::Expression(expression) => expression,
        };
        if expression.get_dependencies().intersects(Dependency::FEATURE | Dependency::IMAGE) {
            return Self::per_feature(expression, parameters, integer_zoom, default);
        }
        let z = if integer_zoom { parameters.z.floor() } else { parameters.z };
        let context = EvaluationContext::new(z).with_global_state(parameters.global_state.as_deref());
        Evaluated::Constant(expression.evaluate(&context, default))
    }

    /// To the values at two zooms to fade between: `line-dasharray` and
    /// `background-pattern`.
    #[inline(never)]
    pub fn cross_faded(value: &PropertyValue<T>, parameters: &PropertyEvaluationParameters, default: T) -> Self {
        let expression = match value {
            PropertyValue::Undefined => return Self::cross_fade(parameters, default.clone(), default.clone(), default),
            PropertyValue::Constant(constant) => {
                return Self::cross_fade(parameters, constant.clone(), constant.clone(), constant.clone());
            }
            PropertyValue::Expression(expression) => expression,
        };
        let global_state = parameters.global_state.as_deref();
        let at = |z: f32| expression.evaluate(&EvaluationContext::new(z).with_global_state(global_state), T::default());
        let z = parameters.z;
        Self::cross_fade(parameters, at(z - 1.0), at(z), at(z + 1.0))
    }

    /// As [`Evaluated::cross_faded`], or to an expression for each feature when
    /// the value reads the feature or the images: the patterns.
    #[inline(never)]
    pub fn cross_faded_per_feature(
        value: &PropertyValue<T>,
        parameters: &PropertyEvaluationParameters,
        default: T,
    ) -> Self {
        let value = match value {
            PropertyValue::Undefined => default,
            PropertyValue::Constant(constant) => constant.clone(),
            PropertyValue::Expression(expression) => {
                if !expression.is_feature_constant() || !expression.is_runtime_constant() {
                    return Self::per_feature(expression, parameters, false, default);
                }
                expression.evaluate(&EvaluationContext::new(parameters.z.floor()), T::default())
            }
        };
        Self::cross_fade(parameters, value.clone(), value.clone(), value)
    }

    /// As [`Evaluated::cross_faded`], or to an expression for each feature
    /// when the value reads the feature: `line-dasharray`.
    pub fn cross_faded_or_per_feature(
        value: &PropertyValue<T>,
        parameters: &PropertyEvaluationParameters,
        default: T,
    ) -> Self {
        if let PropertyValue::Expression(expression) = value
            && !expression.is_feature_constant()
        {
            return Self::per_feature(expression, parameters, false, default);
        }
        Self::cross_faded(value, parameters, default)
    }

    /// Fades to `mid` from `min` when zooming in, or from `max` when zooming
    /// out.
    fn cross_fade(parameters: &PropertyEvaluationParameters, min: T, mid: T, max: T) -> Self {
        let from = if parameters.z > parameters.zoom_history.last_integer_zoom { min } else { max };
        Evaluated::CrossFaded(Faded { from, to: mid })
    }

    fn per_feature(
        expression: &PropertyExpression<T>,
        parameters: &PropertyEvaluationParameters,
        integer_zoom: bool,
        default: T,
    ) -> Self {
        let mut expression = expression.clone();
        expression.set_use_integer_zoom(integer_zoom);
        expression.capture_global_state(parameters.global_state.clone());
        Evaluated::PerFeature { expression, default }
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
