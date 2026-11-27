use std::sync::Arc;

use mln_wire::shared;

use crate::collections::VecSet;
use crate::expression::value::from_expression_values;
use crate::expression::{
    Dependency, EvaluationContext, Expr, GlobalStateMap, Value, ValueConverter, ZoomCurvePtr,
    collect_global_state_refs, find_zoom_curve_checked,
};
use crate::range::Range;
use crate::wire::SharedExpression;

fn collect_global_state_refs_shared(expression: &Expr) -> Option<Arc<VecSet<String>>> {
    if !expression.has(Dependency::GLOBAL_STATE) {
        return None;
    }
    let mut refs = VecSet::new();
    collect_global_state_refs(expression, &mut refs);
    Some(Arc::new(refs))
}

/// A shared expression plus its precomputed constness flags. The zoom curve
/// isn't cached, since it would borrow from the expression:
/// `get_zoom_curve` finds it each time.
#[derive(Clone, Debug)]
pub struct PropertyExpressionBase {
    expression: Arc<Expr>,
    captured_global_state: Option<Arc<GlobalStateMap>>,
    global_state_refs: Option<Arc<VecSet<String>>>,
    use_integer_zoom: bool,
    global_state_pinned: bool,
    is_zoom_constant: bool,
    is_feature_constant: bool,
    is_runtime_constant: bool,
}

impl PropertyExpressionBase {
    pub(crate) fn new(expression: Arc<Expr>) -> Self {
        let global_state_refs = collect_global_state_refs_shared(expression.as_ref());
        let is_zoom_constant = !expression.has(Dependency::ZOOM);
        let is_feature_constant = !expression.has(Dependency::FEATURE);
        let is_runtime_constant = !expression.has(Dependency::IMAGE | Dependency::GLOBAL_STATE);
        PropertyExpressionBase {
            expression,
            captured_global_state: None,
            global_state_refs,
            use_integer_zoom: false,
            global_state_pinned: false,
            is_zoom_constant,
            is_feature_constant,
            is_runtime_constant,
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
    pub(crate) fn is_runtime_constant(&self) -> bool {
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
    pub fn get_expression(&self) -> &Expr {
        self.expression.as_ref()
    }

    /// The expression, as the `Arc` that values of the property share.
    #[must_use]
    pub fn shared_expression(&self) -> &Arc<Expr> {
        &self.expression
    }

    #[must_use]
    pub fn get_use_integer_zoom(&self) -> bool {
        self.use_integer_zoom
    }

    pub(crate) fn set_use_integer_zoom(&mut self, value: bool) {
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
    pub(crate) fn pin_global_state(&mut self, global_state: Arc<GlobalStateMap>) {
        if self.global_state_pinned {
            return;
        }
        self.captured_global_state = Some(global_state);
        self.global_state_pinned = true;
    }

    #[must_use]
    pub(crate) fn get_captured_global_state(&self) -> Option<&Arc<GlobalStateMap>> {
        self.captured_global_state.as_ref()
    }

    /// None if the expression does not reference the global state.
    #[must_use]
    pub(crate) fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        self.global_state_refs.as_deref()
    }

    #[must_use]
    pub(crate) fn get_dependencies(&self) -> Dependency {
        self.expression.dependencies()
    }

    /// The expression's zoom curve (recomputed, see the struct comment).
    #[must_use]
    pub(crate) fn get_zoom_curve(&self) -> ZoomCurvePtr<'_> {
        if self.expression.has(Dependency::ZOOM) {
            find_zoom_curve_checked(self.expression.as_ref())
        } else {
            ZoomCurvePtr::Null
        }
    }
}

/// Port-only: crosses as the expression (shared, see wire.rs)
/// and what was set on it; the flags are derived from the expression again.
///
/// Not inlined: every `PropertyExpression<T>`, `PropertyValue<T>` and
/// `Evaluated<T>` reads and writes one, so inlined it would compile once per
/// value type.
impl mln_wire::Wire for PropertyExpressionBase {
    #[inline(never)]
    fn write(&self, w: &mut mln_wire::Writer) {
        shared::write::<SharedExpression>(&self.expression, w);
        self.captured_global_state.write(w);
        self.use_integer_zoom.write(w);
        self.global_state_pinned.write(w);
    }

    #[inline(never)]
    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        let mut base = PropertyExpressionBase::new(shared::read::<SharedExpression>(r)?);
        base.captured_global_state = mln_wire::Wire::read(r)?;
        base.use_integer_zoom = mln_wire::Wire::read(r)?;
        base.global_state_pinned = mln_wire::Wire::read(r)?;
        Some(base)
    }
}

#[derive(Clone, Debug, mln_wire::Wire)]
pub struct PropertyExpression<T> {
    base: PropertyExpressionBase,
    /// A legacy function's `default`, for values its expression can't
    /// convert to `T`, such as a string that names no enum value. Held as
    /// a value rather than a `T`, so that only one copy of the code that
    /// clones and sends it is compiled.
    default_value: Option<Value>,
    output: std::marker::PhantomData<fn() -> T>,
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
    #[must_use]
    pub fn new(expression: Arc<Expr>) -> Self {
        PropertyExpression {
            base: PropertyExpressionBase::new(expression),
            default_value: None,
            output: std::marker::PhantomData,
        }
    }

    #[must_use]
    pub fn from_expr(expression: Expr) -> Self {
        PropertyExpression::new(Arc::new(expression))
    }

    /// Evaluates to `default_value` where the expression fails or its value
    /// doesn't convert to `T`, rather than to the property's default.
    #[must_use]
    pub fn with_default(self, default_value: Option<T>) -> Self
    where
        T: ValueConverter,
    {
        PropertyExpression { default_value: default_value.map(|d| d.to_expression_value()), ..self }
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

    pub(crate) fn possible_outputs(&self) -> Vec<Option<T>> {
        from_expression_values::<T>(&self.base.expression.possible_outputs())
    }

    fn evaluate_context(&self, context: &EvaluationContext<'_>, final_default_value: T) -> T {
        T::evaluate(&self.base.expression, context)
            .or_else(|| self.default_value.as_ref().and_then(T::from_expression_value))
            .unwrap_or(final_default_value)
    }
}

impl<T> PartialEq for PropertyExpression<T> {
    fn eq(&self, other: &Self) -> bool {
        self.base.expression == other.base.expression
    }
}
