// C++ `Expression` is an abstract base class with `kind`, `type` and
// `dependencies` fields; here every expression type embeds an
// `ExpressionBase` and implements the `Expression` trait. C++ downcasts with
// `getKind()` + `static_cast`; the port uses `downcast` (`Any`).

use std::any::Any;
use std::borrow::Cow;
use std::fmt;

use feature::{Feature, Identifier as FeatureIdentifier, Value as MbglValue, ValueObject};
use mln_text::glyph::FontFaces;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::geometry::{FeatureType, to_feature_type};
use mln_vec_map::VecSet;

use crate::compound_expression::CompoundExpression;
use crate::literal::Literal;
use crate::type_::Type;
use crate::value::{Value, from_mbgl_value};

pub type FeatureState = ValueObject;
pub type GlobalStateMap = ValueObject;

#[derive(Clone, Debug, Default, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct EvaluationError {
    pub message: String,
}

impl EvaluationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        EvaluationError { message: message.into() }
    }
}

pub(crate) type EvalResult<T> = Result<T, EvaluationError>;

pub type EvaluationResult = Result<Value, EvaluationError>;

/// The inputs an expression is evaluated with.
#[derive(Clone, Copy, Default)]
pub struct EvaluationContext<'a> {
    pub zoom: Option<f32>,
    pub accumulated: Option<&'a MbglValue>,
    pub feature: Option<&'a dyn GeometryTileFeature>,
    pub color_ramp_parameter: Option<f64>,
    pub elevation: Option<f32>,
    /// Contains formatted section object, `std::unordered_map<std::string, Value>`.
    pub formatted_section: Option<&'a Value>,
    pub feature_state: Option<&'a FeatureState>,
    pub available_images: Option<&'a VecSet<String>>,
    pub canonical: Option<&'a CanonicalTileID>,
    pub global_state: Option<&'a GlobalStateMap>,
    /// The style's font faces, which let "is-supported-script" accept the
    /// scripts that they cover.
    pub font_faces: Option<&'a FontFaces>,
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
    pub fn with_font_faces(mut self, font_faces: Option<&'a FontFaces>) -> Self {
        self.font_faces = font_faces;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Kind {
    Coalesce,
    CompoundExpression,
    Literal,
    At,
    Interpolate,
    Assertion,
    Length,
    Step,
    Let,
    Var,
    CollatorExpression,
    Coercion,
    Match,
    Error,
    Case,
    Any,
    All,
    Comparison,
    FormatExpression,
    FormatSectionOverride,
    NumberFormat,
    ImageExpression,
    In,
    Within,
    Distance,
    IndexOf,
    Slice,
    Semiliteral,
}

/// The inputs an expression depends on, as a bit set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Dependency(pub u32);

impl Dependency {
    pub const NONE: Dependency = Dependency(0);
    /// Data reference
    pub const FEATURE: Dependency = Dependency(1 << 0);
    /// Image reference (equivalent to not "runtime constant")
    pub const IMAGE: Dependency = Dependency(1 << 1);
    /// Zoom level
    pub const ZOOM: Dependency = Dependency(1 << 2);
    /// Not used yet, "distance-from-center" not supported
    pub(crate) const LOCATION: Dependency = Dependency(1 << 3);
    /// Create variable binding ("let")
    pub const BIND: Dependency = Dependency(1 << 4);
    /// Use variable binding
    pub const VAR: Dependency = Dependency(1 << 5);
    /// Property override
    pub const OVERRIDE: Dependency = Dependency(1 << 6);
    /// Elevation from DEM
    pub const ELEVATION: Dependency = Dependency(1 << 7);
    /// Global state ("global-state")
    pub const GLOBAL_STATE: Dependency = Dependency(1 << 8);

    /// `bool(a & b)` (`util/bitmask_operations.hpp`).
    #[must_use]
    pub fn intersects(self, other: Dependency) -> bool {
        (self.0 & other.0) != 0
    }
}

impl std::ops::BitOr for Dependency {
    type Output = Dependency;
    fn bitor(self, rhs: Dependency) -> Dependency {
        Dependency(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Dependency {
    fn bitor_assign(&mut self, rhs: Dependency) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Dependency::NONE {
            return f.write_str("None");
        }
        let mut any = false;
        for (dep, name) in [
            (Dependency::FEATURE, "Feature"),
            (Dependency::IMAGE, "Image"),
            (Dependency::ZOOM, "Zoom"),
            (Dependency::LOCATION, "Location"),
            (Dependency::BIND, "Bind"),
            (Dependency::VAR, "Var"),
            (Dependency::OVERRIDE, "Override"),
            (Dependency::ELEVATION, "Elevation"),
            (Dependency::GLOBAL_STATE, "GlobalState"),
        ] {
            if self.intersects(dep) {
                if any {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                any = true;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The fields of the C++ `Expression` base class.
#[derive(Clone, Debug)]
pub struct ExpressionBase {
    pub(crate) dependencies: Dependency,
    kind: Kind,
    type_: Type,
}

impl ExpressionBase {
    #[must_use]
    pub fn new(kind: Kind, type_: Type, dependencies: Dependency) -> Self {
        ExpressionBase { dependencies, kind, type_ }
    }
}

pub trait Expression: Any + Send + Sync {
    fn base(&self) -> &ExpressionBase;

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult;

    /// The value a literal always evaluates to, which callers can borrow
    /// instead of cloning it on each evaluation. `None` for any other
    /// expression.
    fn literal_value(&self) -> Option<&Value> {
        None
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression));

    /// `operator==`.
    fn equals(&self, other: &dyn Expression) -> bool;

    /// Statically analyze the expression, attempting to enumerate possible
    /// outputs. Returns an array of values plus the sentinel null optional
    /// value, used to indicate that the complete set of outputs is
    /// statically undecidable.
    fn possible_outputs(&self) -> Vec<Option<Value>>;

    fn serialize(&self) -> MbglValue {
        default_serialize(self)
    }

    fn get_operator(&self) -> String;

    fn get_kind(&self) -> Kind {
        self.base().kind
    }

    fn get_type(&self) -> &Type {
        &self.base().type_
    }

    fn dependencies(&self) -> Dependency {
        self.base().dependencies
    }

    /// Test the expression's dependencies for any of one or more values.
    fn has(&self, dep: Dependency) -> bool {
        self.base().dependencies.intersects(dep)
    }
}

/// The base-class `Expression::serialize()`.
pub(crate) fn default_serialize<E: Expression + ?Sized>(e: &E) -> MbglValue {
    let mut serialized = vec![MbglValue::String(e.get_operator())];
    e.each_child(&mut |child| serialized.push(child.serialize()));
    MbglValue::Array(serialized)
}

impl PartialEq for dyn Expression {
    fn eq(&self, other: &dyn Expression) -> bool {
        self.equals(other)
    }
}

impl fmt::Debug for dyn Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Expression({:?})", self.serialize())
    }
}

/// `e` as a `T`, if it is one.
pub fn downcast<T: Expression>(e: &dyn Expression) -> Option<&T> {
    let any: &dyn Any = e;
    any.downcast_ref()
}

/// The union of the dependencies of `container`'s expressions.
pub(crate) fn collect_dependencies<'a, I, E>(container: I) -> Dependency
where
    I: IntoIterator<Item = &'a E>,
    E: std::ops::Deref<Target = dyn Expression> + 'a,
{
    container.into_iter().fold(Dependency::NONE, |a, e| a | e.dependencies())
}

/// Whether two lists of expressions are equal, element by element.
pub(crate) fn children_equal<E, F>(lhs: &[E], rhs: &[F]) -> bool
where
    E: std::ops::Deref<Target = dyn Expression>,
    F: std::ops::Deref<Target = dyn Expression>,
{
    if lhs.len() != rhs.len() {
        return false;
    }
    lhs.iter().zip(rhs).all(|(l, r)| l.equals(&**r))
}

/// Collect the names of the global-state properties referenced by
/// "global-state" expressions in the given expression tree.
pub fn collect_global_state_refs(expression: &dyn Expression, refs: &mut VecSet<String>) {
    if !expression.has(Dependency::GLOBAL_STATE) {
        return;
    }
    if expression.get_kind() == Kind::CompoundExpression {
        let compound = downcast::<CompoundExpression>(expression).expect("CompoundExpression");
        if compound.get_operator() == "global-state" {
            // The property name argument is enforced to be a string literal
            // at parse time.
            expression.each_child(&mut |child| {
                if child.get_kind() == Kind::Literal {
                    let value = downcast::<Literal>(child).expect("Literal").get_value();
                    if let Value::String(s) = value {
                        refs.insert(s.clone());
                    }
                }
            });
            return;
        }
    }
    expression.each_child(&mut |child| collect_global_state_refs(child, refs));
}

/// Whether any of the global-state keys referenced by an expression is among
/// the changed keys. A null set on either side means "unknown" and is
/// conservatively treated as a match.
#[must_use]
pub fn global_state_refs_intersect(refs: Option<&VecSet<String>>, changed_keys: Option<&VecSet<String>>) -> bool {
    match (refs, changed_keys) {
        (Some(refs), Some(changed_keys)) => refs.iter().any(|key| changed_keys.contains(key)),
        _ => true,
    }
}

/// The `GeoJSONFeature` adapter of expression.cpp: a `GeometryTileFeature`
/// view of an `mln::Feature`.
struct GeoJSONFeature<'a> {
    feature: &'a Feature<f64>,
    geometry: GeometryCollection,
}

impl<'a> GeoJSONFeature<'a> {
    fn new(feature: &'a Feature<f64>) -> Self {
        GeoJSONFeature { feature, geometry: GeometryCollection::default() }
    }

    fn with_canonical(feature: &'a Feature<f64>, canonical: &CanonicalTileID) -> Self {
        let geometry = feature.geometry.as_ref().map_or_else(GeometryCollection::default, |g| {
            mln_tile::geometry_tile_data::convert_geometry_to_tile(g, canonical)
        });
        GeoJSONFeature { feature, geometry }
    }
}

impl GeometryTileFeature for GeoJSONFeature<'_> {
    fn get_type(&self) -> FeatureType {
        to_feature_type(self.feature.geometry.as_ref())
    }

    fn get_properties(&self) -> &ValueObject {
        &self.feature.properties
    }

    fn get_id(&self) -> FeatureIdentifier {
        self.feature.id.clone()
    }

    fn get_value(&self, key: &str) -> Option<MbglValue> {
        self.feature.properties.get(key).cloned()
    }

    fn get_geometries(&self) -> &GeometryCollection {
        &self.geometry
    }
}

#[cfg(test)]
pub(crate) fn evaluate_feature(
    e: &dyn Expression,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(&EvaluationContext::with_color_ramp(zoom, Some(&f), color_ramp_parameter))
}

pub fn evaluate_feature_with_images(
    e: &dyn Expression,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
    available_images: &VecSet<String>,
) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(
        &EvaluationContext::with_color_ramp(zoom, Some(&f), color_ramp_parameter)
            .with_available_images(Some(available_images)),
    )
}

pub fn evaluate_feature_with_canonical(
    e: &dyn Expression,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
    available_images: &VecSet<String>,
    canonical: &CanonicalTileID,
) -> EvaluationResult {
    let f = GeoJSONFeature::with_canonical(feature, canonical);
    e.evaluate(
        &EvaluationContext::with_color_ramp(zoom, Some(&f), color_ramp_parameter)
            .with_available_images(Some(available_images))
            .with_canonical_tile_id(Some(canonical)),
    )
}

pub fn evaluate_accumulated(
    e: &dyn Expression,
    accumulated: Option<&MbglValue>,
    feature: &Feature<f64>,
) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(&EvaluationContext::from_accumulated(accumulated, Some(&f)))
}

/// Evaluates `e` against a GeoJSON feature and every other input of `params`,
/// whose own feature is ignored.
/// When `params` names a canonical tile, the feature's geometry is placed in
/// that tile, as the within and distance expressions need.
pub fn evaluate_feature_with_context(
    e: &dyn Expression,
    feature: &Feature<f64>,
    params: EvaluationContext<'_>,
) -> EvaluationResult {
    let f = match params.canonical {
        Some(canonical) => GeoJSONFeature::with_canonical(feature, canonical),
        None => GeoJSONFeature::new(feature),
    };
    e.evaluate(&EvaluationContext { feature: Some(&f), ..params })
}

/// `evaluate`, borrowing a literal's value instead of cloning it.
pub(crate) fn evaluate_ref<'a>(
    expression: &'a dyn Expression,
    params: &EvaluationContext<'_>,
) -> Result<Cow<'a, Value>, EvaluationError> {
    match expression.literal_value() {
        Some(value) => Ok(Cow::Borrowed(value)),
        None => expression.evaluate(params).map(Cow::Owned),
    }
}

/// Converts a feature value to an expression value, for the feature
/// accessors.
pub(crate) fn mbgl_to_expression_value(v: &MbglValue) -> Value {
    from_mbgl_value(v)
}
