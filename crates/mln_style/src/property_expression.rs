// Ports include/mln/style/property_expression.hpp +
// src/mln/style/property_expression.cpp.

use std::sync::Arc;

use mln_expression::expression::FeatureState;
use mln_expression::range::Range;
use mln_expression::value::from_expression_values;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_vec_map::VecSet;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::expression::{
    Dependency, EvaluationContext, Expression, GlobalStateMap, Type, ValueConverter, ZoomCurvePtr,
    collect_global_state_refs, find_zoom_curve_checked,
};
use crate::serialization::shared_expression;

fn check_gpu_capable(expression: &dyn Expression, zoom_curve: &ZoomCurvePtr<'_>) -> bool {
    expression.dependencies() == Dependency::ZOOM
        && !zoom_curve.is_null()
        && (*expression.get_type() == Type::Number || *expression.get_type() == Type::Color)
}

fn collect_global_state_refs_shared(expression: &dyn Expression) -> Option<Arc<VecSet<String>>> {
    if !expression.has(Dependency::GLOBAL_STATE) {
        return None;
    }
    let mut refs = VecSet::new();
    collect_global_state_refs(expression, &mut refs);
    Some(Arc::new(refs))
}

/// `PropertyExpressionBase`: a shared expression plus its precomputed
/// constness flags.
///
/// C++ also caches `zoomCurve` (raw pointers into the shared expression).
/// A Rust struct cannot hold borrows of its own `Arc`, so
/// [`get_zoom_curve`](Self::get_zoom_curve) recomputes it with
/// `findZoomCurveChecked` (a pure function of the expression).
#[derive(Debug)]
pub struct PropertyExpressionBase {
    expression: Arc<dyn Expression>,
    captured_global_state: Option<Arc<GlobalStateMap>>,
    global_state_refs: Option<Arc<VecSet<String>>>,
    use_integer_zoom: bool,
    global_state_pinned: bool,
    is_zoom_constant: bool,
    is_feature_constant: bool,
    is_runtime_constant: bool,
    // If the expression depends on zoom and nothing else, and produces
    // a number or color, we can potentially evaluate it on the GPU
    is_gpu_capable: bool,
}

// Not derived, so that it can be `#[inline(never)]`: see `PropertyValue`'s
// Clone.
impl Clone for PropertyExpressionBase {
    #[inline(never)]
    fn clone(&self) -> Self {
        PropertyExpressionBase {
            expression: self.expression.clone(),
            captured_global_state: self.captured_global_state.clone(),
            global_state_refs: self.global_state_refs.clone(),
            use_integer_zoom: self.use_integer_zoom,
            global_state_pinned: self.global_state_pinned,
            is_zoom_constant: self.is_zoom_constant,
            is_feature_constant: self.is_feature_constant,
            is_runtime_constant: self.is_runtime_constant,
            is_gpu_capable: self.is_gpu_capable,
        }
    }
}

impl PropertyExpressionBase {
    pub fn new(expression: Arc<dyn Expression>) -> Self {
        let global_state_refs = collect_global_state_refs_shared(expression.as_ref());
        let is_zoom_constant = !expression.has(Dependency::ZOOM);
        let is_feature_constant = !expression.has(Dependency::FEATURE);
        let is_runtime_constant = !expression.has(Dependency::IMAGE | Dependency::GLOBAL_STATE);
        let is_gpu_capable = {
            let zoom_curve = if expression.has(Dependency::ZOOM) {
                find_zoom_curve_checked(expression.as_ref())
            } else {
                ZoomCurvePtr::Null
            };
            check_gpu_capable(expression.as_ref(), &zoom_curve)
        };
        debug_assert_eq!(is_zoom_constant, crate::expression::is_zoom_constant(expression.as_ref()));
        debug_assert_eq!(is_feature_constant, crate::expression::is_feature_constant(expression.as_ref()));
        debug_assert_eq!(is_runtime_constant, crate::expression::is_runtime_constant(expression.as_ref()));
        PropertyExpressionBase {
            expression,
            captured_global_state: None,
            global_state_refs,
            use_integer_zoom: false,
            global_state_pinned: false,
            is_zoom_constant,
            is_feature_constant,
            is_runtime_constant,
            is_gpu_capable,
        }
    }

    #[must_use]
    pub fn is_zoom_constant(&self) -> bool {
        self.is_zoom_constant
    }

    #[must_use]
    pub fn is_feature_constant(&self) -> bool {
        self.is_feature_constant
    }

    #[must_use]
    pub fn is_runtime_constant(&self) -> bool {
        self.is_runtime_constant
    }

    #[must_use]
    pub fn interpolation_factor(&self, input_levels: Range<f32>, input_value: f32) -> f32 {
        match self.get_zoom_curve() {
            ZoomCurvePtr::Null => {
                debug_assert!(false);
                0.0
            }
            ZoomCurvePtr::Interpolate(z) => z.interpolation_factor(
                Range::new(f64::from(input_levels.min), f64::from(input_levels.max)),
                f64::from(input_value),
            ) as f32,
            ZoomCurvePtr::Step(_) => 0.0,
        }
    }

    #[must_use]
    pub fn get_covering_stops(&self, lower: f32, upper: f32) -> Range<f32> {
        match self.get_zoom_curve() {
            ZoomCurvePtr::Null => {
                debug_assert!(false);
                Range::new(0.0, 0.0)
            }
            ZoomCurvePtr::Interpolate(z) => z.get_covering_stops(f64::from(lower), f64::from(upper)),
            ZoomCurvePtr::Step(z) => z.get_covering_stops(f64::from(lower), f64::from(upper)),
        }
    }

    #[must_use]
    pub fn get_expression(&self) -> &dyn Expression {
        self.expression.as_ref()
    }

    #[must_use]
    pub fn is_gpu_capable(&self) -> bool {
        self.is_gpu_capable
    }

    #[must_use]
    pub fn get_use_integer_zoom(&self) -> bool {
        self.use_integer_zoom
    }

    pub fn set_use_integer_zoom(&mut self, value: bool) {
        self.use_integer_zoom = value;
    }

    /// Capture a snapshot of the style's global state so that later
    /// evaluations (e.g. per-feature evaluation during tile layout or in
    /// paint property binders) can resolve `global-state` expressions without
    /// explicit plumbing. Only retained if the expression actually depends on
    /// it.
    pub fn capture_global_state(&mut self, global_state: Option<Arc<GlobalStateMap>>) {
        if self.expression.has(Dependency::GLOBAL_STATE) && !self.global_state_pinned {
            self.captured_global_state = global_state;
        }
    }

    /// Evaluate `global-state` with this snapshot even when the evaluation
    /// context carries a state of its own, so a value shown before a global
    /// state change keeps its look while it transitions out. The first pin
    /// wins: a value already showing an older state keeps showing it.
    pub fn pin_global_state(&mut self, global_state: Arc<GlobalStateMap>) {
        if self.global_state_pinned {
            return;
        }
        self.captured_global_state = Some(global_state);
        self.global_state_pinned = true;
    }

    #[must_use]
    pub fn get_captured_global_state(&self) -> Option<&Arc<GlobalStateMap>> {
        self.captured_global_state.as_ref()
    }

    /// None if the expression does not reference the global state.
    #[must_use]
    pub fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        self.global_state_refs.as_deref()
    }

    /// Can be used for aggregating property expressions from multiple
    /// properties(layers) into single match / case expression.
    #[must_use]
    pub fn get_shared_expression(&self) -> Arc<dyn Expression> {
        self.expression.clone()
    }

    // TODO(port): getGPUExpression (gfx::GPUExpression) with the renderer's
    // GPU-expression path.

    #[must_use]
    pub fn get_dependencies(&self) -> Dependency {
        self.expression.dependencies()
    }

    /// `getZoomCurve()` (recomputed, see the struct comment).
    #[must_use]
    pub fn get_zoom_curve(&self) -> ZoomCurvePtr<'_> {
        if self.expression.has(Dependency::ZOOM) {
            find_zoom_curve_checked(self.expression.as_ref())
        } else {
            ZoomCurvePtr::Null
        }
    }
}

/// Port-only: serde writes the expression (shared, see
/// serialization.rs) and what was set on it; the flags are
/// derived from the expression again.
#[derive(Serialize, Deserialize)]
struct PropertyExpressionParts {
    #[serde(with = "shared_expression")]
    expression: Arc<dyn Expression>,
    captured_global_state: Option<Arc<GlobalStateMap>>,
    use_integer_zoom: bool,
    global_state_pinned: bool,
}

impl Serialize for PropertyExpressionBase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        PropertyExpressionParts {
            expression: self.expression.clone(),
            captured_global_state: self.captured_global_state.clone(),
            use_integer_zoom: self.use_integer_zoom,
            global_state_pinned: self.global_state_pinned,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PropertyExpressionBase {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let parts = PropertyExpressionParts::deserialize(deserializer)?;
        let mut base = PropertyExpressionBase::new(parts.expression);
        base.captured_global_state = parts.captured_global_state;
        base.use_integer_zoom = parts.use_integer_zoom;
        base.global_state_pinned = parts.global_state_pinned;
        Ok(base)
    }
}

/// `PropertyExpression<T>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PropertyExpression<T> {
    base: PropertyExpressionBase,
    default_value: Option<T>,
}

impl<T> std::ops::Deref for PropertyExpression<T> {
    type Target = PropertyExpressionBase;
    fn deref(&self) -> &PropertyExpressionBase {
        &self.base
    }
}

impl<T> std::ops::DerefMut for PropertyExpression<T> {
    fn deref_mut(&mut self) -> &mut PropertyExpressionBase {
        &mut self.base
    }
}

impl<T> PropertyExpression<T> {
    /// Second parameter to be used only for conversions from legacy functions.
    pub fn new(expression: Arc<dyn Expression>, default_value: Option<T>) -> Self {
        PropertyExpression { base: PropertyExpressionBase::new(expression), default_value }
    }

    /// `PropertyExpression(std::unique_ptr<Expression>, ...)`.
    pub fn from_box(expression: Box<dyn Expression>, default_value: Option<T>) -> Self {
        PropertyExpression::new(Arc::from(expression), default_value)
    }
}

impl<T: Clone + ValueConverter> PropertyExpression<T> {
    pub fn evaluate(&self, context: &EvaluationContext<'_>, final_default_value: T) -> T {
        // A pinned snapshot wins, and the captured one fills in for a context
        // that carries no state.
        match &self.base.captured_global_state {
            Some(captured) if self.base.global_state_pinned || context.global_state.is_none() => {
                let context_with_state = context.with_global_state(Some(captured.as_ref()));
                self.evaluate_context(&context_with_state, final_default_value)
            }
            _ => self.evaluate_context(context, final_default_value),
        }
    }

    /// `evaluate(float zoom)`.
    pub fn evaluate_zoom(&self, zoom: f32) -> T
    where
        T: Default,
    {
        debug_assert!(!self.is_zoom_constant() || self.get_dependencies().intersects(Dependency::GLOBAL_STATE));
        debug_assert!(self.is_feature_constant());
        self.evaluate(&EvaluationContext::new(zoom), T::default())
    }

    /// `evaluate(const GeometryTileFeature&, T finalDefaultValue)`.
    pub fn evaluate_feature(&self, feature: &dyn GeometryTileFeature, final_default_value: T) -> T {
        debug_assert!(self.is_zoom_constant());
        debug_assert!(!self.is_feature_constant());
        self.evaluate(&EvaluationContext::from_feature(Some(feature)), final_default_value)
    }

    /// `evaluate(feature, availableImages, finalDefaultValue)`.
    pub fn evaluate_feature_images(
        &self,
        feature: &dyn GeometryTileFeature,
        available_images: &VecSet<String>,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::from_feature(Some(feature)).with_available_images(Some(available_images)),
            final_default_value,
        )
    }

    /// `evaluate(feature, canonical, finalDefaultValue)`.
    pub fn evaluate_feature_canonical(
        &self,
        feature: &dyn GeometryTileFeature,
        canonical: &CanonicalTileID,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::from_feature(Some(feature)).with_canonical_tile_id(Some(canonical)),
            final_default_value,
        )
    }

    /// `evaluate(feature, availableImages, canonical, finalDefaultValue)`.
    pub fn evaluate_feature_images_canonical(
        &self,
        feature: &dyn GeometryTileFeature,
        available_images: &VecSet<String>,
        canonical: &CanonicalTileID,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::from_feature(Some(feature))
                .with_available_images(Some(available_images))
                .with_canonical_tile_id(Some(canonical)),
            final_default_value,
        )
    }

    /// `evaluate(float zoom, const GeometryTileFeature&, T finalDefaultValue)`.
    pub fn evaluate_zoom_feature(&self, zoom: f32, feature: &dyn GeometryTileFeature, final_default_value: T) -> T {
        self.evaluate(&EvaluationContext::with_zoom_feature(zoom, Some(feature)), final_default_value)
    }

    /// `evaluate(zoom, feature, availableImages, finalDefaultValue)`.
    pub fn evaluate_zoom_feature_images(
        &self,
        zoom: f32,
        feature: &dyn GeometryTileFeature,
        available_images: &VecSet<String>,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_available_images(Some(available_images)),
            final_default_value,
        )
    }

    /// `evaluate(zoom, feature, availableImages, canonical, finalDefaultValue)`.
    pub fn evaluate_zoom_feature_images_canonical(
        &self,
        zoom: f32,
        feature: &dyn GeometryTileFeature,
        available_images: &VecSet<String>,
        canonical: &CanonicalTileID,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                .with_available_images(Some(available_images))
                .with_canonical_tile_id(Some(canonical)),
            final_default_value,
        )
    }

    /// `evaluate(zoom, feature, canonical, finalDefaultValue)`.
    pub fn evaluate_zoom_feature_canonical(
        &self,
        zoom: f32,
        feature: &dyn GeometryTileFeature,
        canonical: &CanonicalTileID,
        final_default_value: T,
    ) -> T {
        self.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
            final_default_value,
        )
    }

    /// `evaluate(zoom, feature, const FeatureState&, finalDefaultValue)`.
    pub fn evaluate_zoom_feature_state(
        &self,
        zoom: f32,
        feature: &dyn GeometryTileFeature,
        state: &FeatureState,
        final_default_value: T,
    ) -> T {
        debug_assert!(!self.is_feature_constant());
        self.evaluate(
            &EvaluationContext::with_zoom_feature_state(zoom, Some(feature), Some(state)),
            final_default_value,
        )
    }

    pub fn possible_outputs(&self) -> Vec<Option<T>> {
        from_expression_values::<T>(&self.base.expression.possible_outputs())
    }

    fn evaluate_context(&self, context: &EvaluationContext<'_>, final_default_value: T) -> T {
        if let Ok(result) = self.base.expression.evaluate(context)
            && let Some(typed) = T::from_expression_value(&result)
        {
            return typed;
        }
        match &self.default_value {
            Some(default_value) => default_value.clone(),
            None => final_default_value,
        }
    }
}

impl<T> PartialEq for PropertyExpression<T> {
    fn eq(&self, other: &Self) -> bool {
        self.base.expression.equals(other.base.expression.as_ref())
    }
}
