//! Evaluating an expression: its inputs ([`EvaluationContext`]), why it
//! fails ([`EvaluationError`]), and evaluation against a GeoJSON feature.

use std::fmt;

use crate::collections::VecSet;
use crate::expression::Expr;
use crate::expression::forms::image::UnavailableImages;
use crate::expression::type_::Type;
use crate::expression::value::{Value, from_mbgl_value, type_of};
use crate::feature::geojson_feature::GeoJSONFeature;
use crate::feature::{Feature, FeatureState, GeometryTileFeature, Value as MbglValue, ValueObject};
use crate::font_face::FontCoverage;
use crate::tile_id::CanonicalTileID;

pub type GlobalStateMap = ValueObject;

/// Why an expression failed to evaluate. One pointer wide, so that a typed
/// result such as `Result<f64, EvaluationError>` fits in two registers.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.message)]
pub struct EvaluationError(Box<ErrorMessage>);

#[derive(Clone, PartialEq, Eq)]
struct ErrorMessage {
    message: String,
}

impl EvaluationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        EvaluationError(Box::new(ErrorMessage { message: message.into() }))
    }

    /// The error for a value whose type isn't the one parsing checked for.
    pub(crate) fn type_mismatch(expected: &Type, value: &Value) -> Self {
        EvaluationError::new(format!(
            "Expected value to be of type {}, but found {} instead.",
            crate::expression::type_::to_string(expected),
            crate::expression::type_::to_string(&type_of(value))
        ))
    }

    /// The message the style specification's expression tests expect.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0.message
    }
}

impl fmt::Debug for EvaluationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EvaluationError({:?})", self.0.message)
    }
}

/// The result of a typed evaluation.
pub type EvalResult<T> = Result<T, EvaluationError>;

pub type EvaluationResult = Result<Value, EvaluationError>;

/// The inputs an expression is evaluated with.
#[derive(Clone, Copy, Default)]
pub struct EvaluationContext<'a> {
    pub zoom: Option<f32>,
    pub accumulated: Option<&'a MbglValue>,
    pub feature: Option<&'a dyn GeometryTileFeature>,
    pub color_ramp_parameter: Option<f64>,
    pub elevation: Option<f32>,
    /// The section of a `formatted` value that a `format-section-override`
    /// reads, as an object.
    pub formatted_section: Option<&'a Value>,
    pub feature_state: Option<&'a FeatureState>,
    pub available_images: Option<&'a VecSet<String>>,
    /// Where `image` expressions evaluated against `available_images` record
    /// the images they don't find.
    pub unavailable_images: Option<&'a UnavailableImages>,
    pub canonical: Option<&'a CanonicalTileID>,
    pub global_state: Option<&'a GlobalStateMap>,
    /// The fonts the map draws text with, which let "is-supported-script"
    /// accept the scripts that they cover.
    pub font_faces: Option<&'a dyn FontCoverage>,
}

impl<'a> EvaluationContext<'a> {
    #[must_use]
    pub fn new(zoom: f32) -> Self {
        EvaluationContext { zoom: Some(zoom), ..Default::default() }
    }

    #[must_use]
    pub fn from_feature(feature: Option<&'a dyn GeometryTileFeature>) -> Self {
        EvaluationContext { feature, ..Default::default() }
    }

    #[must_use]
    pub fn with_zoom_feature(zoom: f32, feature: Option<&'a dyn GeometryTileFeature>) -> Self {
        EvaluationContext { zoom: Some(zoom), feature, ..Default::default() }
    }

    #[must_use]
    pub(crate) fn from_accumulated(
        accumulated: Option<&'a MbglValue>,
        feature: Option<&'a dyn GeometryTileFeature>,
    ) -> Self {
        EvaluationContext { accumulated, feature, ..Default::default() }
    }

    #[must_use]
    pub fn with_zoom_feature_state(
        zoom: f32,
        feature: Option<&'a dyn GeometryTileFeature>,
        state: Option<&'a FeatureState>,
    ) -> Self {
        EvaluationContext { zoom: Some(zoom), feature, feature_state: state, ..Default::default() }
    }

    #[must_use]
    pub fn with_color_ramp(
        zoom: Option<f32>,
        feature: Option<&'a dyn GeometryTileFeature>,
        color_ramp_parameter: Option<f64>,
    ) -> Self {
        EvaluationContext { zoom, feature, color_ramp_parameter, ..Default::default() }
    }

    #[must_use]
    pub fn with_formatted_section(mut self, formatted_section: Option<&'a Value>) -> Self {
        self.formatted_section = formatted_section;
        self
    }

    #[must_use]
    pub fn with_feature_state(mut self, feature_state: Option<&'a FeatureState>) -> Self {
        self.feature_state = feature_state;
        self
    }

    #[must_use]
    pub fn with_available_images(mut self, available_images: Option<&'a VecSet<String>>) -> Self {
        self.available_images = available_images;
        self
    }

    #[must_use]
    pub fn with_unavailable_images(mut self, unavailable_images: Option<&'a UnavailableImages>) -> Self {
        self.unavailable_images = unavailable_images;
        self
    }

    #[must_use]
    pub fn with_canonical_tile_id(mut self, canonical: Option<&'a CanonicalTileID>) -> Self {
        self.canonical = canonical;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_elevation(mut self, elevation: f32) -> Self {
        self.elevation = Some(elevation);
        self
    }

    #[must_use]
    pub fn with_global_state(mut self, global_state: Option<&'a GlobalStateMap>) -> Self {
        self.global_state = global_state;
        self
    }

    #[must_use]
    pub fn with_font_faces(mut self, font_faces: Option<&'a impl FontCoverage>) -> Self {
        self.font_faces = font_faces.map(|faces| -> &dyn FontCoverage { faces });
        self
    }
}

#[cfg(test)]
pub(crate) fn evaluate_feature(
    e: &Expr,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(&EvaluationContext::with_color_ramp(zoom, Some(&f), color_ramp_parameter))
}

pub fn evaluate_accumulated(e: &Expr, accumulated: Option<&MbglValue>, feature: &Feature<f64>) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(&EvaluationContext::from_accumulated(accumulated, Some(&f)))
}

/// Evaluates `e` against a GeoJSON feature and every other input of `params`,
/// whose own feature is ignored.
/// When `params` names a canonical tile, the feature's geometry is placed in
/// that tile, as the within and distance expressions need.
pub fn evaluate_feature_with_context(
    e: &Expr,
    feature: &Feature<f64>,
    params: EvaluationContext<'_>,
) -> EvaluationResult {
    with_geojson_feature(feature, params, |cx| e.evaluate(cx))
}

/// Calls `f` with `params` and a view of `feature` as its feature, placed in
/// `params`' canonical tile when it names one.
pub fn with_geojson_feature<R>(
    feature: &Feature<f64>,
    params: EvaluationContext<'_>,
    f: impl FnOnce(&EvaluationContext<'_>) -> R,
) -> R {
    let view = match params.canonical {
        Some(canonical) => GeoJSONFeature::with_canonical(feature, canonical),
        None => GeoJSONFeature::new(feature),
    };
    f(&EvaluationContext { feature: Some(&view), ..params })
}

/// Converts a feature value to an expression value, for the feature
/// accessors.
pub(crate) fn mbgl_to_expression_value(v: &MbglValue) -> Value {
    from_mbgl_value(v)
}
