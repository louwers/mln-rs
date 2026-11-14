//! A parsed, type-checked expression ([`Expr`]) and its evaluation.
//!
//! An `Expr` is a closed tree with a fixed set of node kinds. The parser
//! fixes every node's type, so evaluation is typed: a number-typed node
//! evaluates to an `f64` with [`Expr::eval_num`], a filter to a `bool` with
//! [`Expr::eval_bool`], and so on, without building a [`Value`] per node or
//! checking again what the parser proved. [`Expr::evaluate`] gives any node's
//! result as a `Value`, for callers whose type is only known at run time.

use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::discriminant;
use std::sync::Arc;

use feature::{Feature, Identifier as FeatureIdentifier, Value as MbglValue, ValueObject};
use mln_text::glyph::FontFaces;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::Color;
use mln_util::geometry::{FeatureType, to_feature_type};
use mln_vec_map::VecSet;

use crate::assertion::Assertion;
use crate::at::At;
use crate::case::Case;
use crate::coalesce::Coalesce;
use crate::coercion::Coercion;
use crate::collator_expression::CollatorExpression;
use crate::comparison::Comparison;
use crate::compound_expression::{CompoundExpression, Op};
use crate::distance::Distance;
use crate::format_expression::FormatExpression;
use crate::format_section_override::FormatSectionOverride;
use crate::image_expression::ImageExpression;
use crate::in_::In;
use crate::index_of::IndexOf;
use crate::interpolate::Interpolate;
use crate::let_::{Let, Var};
use crate::literal::Literal;
use crate::match_::Match;
use crate::number_format::NumberFormat;
use crate::semiliteral::Semiliteral;
use crate::slice::Slice;
use crate::step::Step;
use crate::type_::Type;
use crate::value::{Value, from_mbgl_value, hash_value, type_of};
use crate::within::Within;

pub type FeatureState = ValueObject;
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
    /// The section of a `formatted` value that a `format-section-override`
    /// reads, as an object.
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
    /// Feature state ("feature-state"). Always set along with `FEATURE`.
    pub const FEATURE_STATE: Dependency = Dependency(1 << 9);
    /// Inputs that only some properties provide while they evaluate:
    /// "heatmap-density", "line-progress" and "accumulated".
    pub const PROPERTY_INPUT: Dependency = Dependency(1 << 10);

    /// Whether the two sets share an input.
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
            (Dependency::FEATURE_STATE, "FeatureState"),
            (Dependency::PROPERTY_INPUT, "PropertyInput"),
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

/// The kinds of expression node. Each holds its own operands.
#[derive(Clone, PartialEq)]
pub(crate) enum Node {
    Literal(Box<Literal>),
    Compound(Box<CompoundExpression>),
    Assertion(Box<Assertion>),
    Coercion(Box<Coercion>),
    Comparison(Box<Comparison>),
    All(Box<[Expr]>),
    Any(Box<[Expr]>),
    At(Box<At>),
    In(Box<In>),
    IndexOf(Box<IndexOf>),
    Slice(Box<Slice>),
    Length(Box<Expr>),
    Case(Box<Case>),
    Coalesce(Box<Coalesce>),
    Match(Box<Match>),
    Step(Box<Step>),
    Interpolate(Box<Interpolate>),
    Let(Box<Let>),
    Var(Box<Var>),
    Collator(Box<CollatorExpression>),
    Format(Box<FormatExpression>),
    Image(Box<ImageExpression>),
    NumberFormat(Box<NumberFormat>),
    Within(Box<Within>),
    Distance(Box<Distance>),
    Semiliteral(Box<Semiliteral>),
    FormatSectionOverride(Box<FormatSectionOverride>),
}

/// A parsed, type-checked expression.
#[derive(Clone)]
pub struct Expr {
    node: Node,
    /// The inputs of the node and its operands.
    deps: Dependency,
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Expression({:?})", self.serialize())
    }
}

/// A result type that typed evaluation produces, so that the nodes that
/// pass on an operand's result (`case`, `match`, `step`, `let`, ...) are
/// written once.
pub(crate) trait Output<'e>: Sized {
    /// Evaluates `e`, whose type is this output's.
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self>;

    /// Converts a value read at run time, from an array or a feature, say.
    fn from_value(value: Value) -> EvalResult<Self>;
}

fn wrong_type(expected: &str, value: &Value) -> EvaluationError {
    EvaluationError::new(format!(
        "Expected value to be of type {expected}, but found {} instead.",
        crate::type_::to_string(&type_of(value))
    ))
}

impl<'e> Output<'e> for f64 {
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self> {
        e.eval_num(cx)
    }
    fn from_value(value: Value) -> EvalResult<Self> {
        match value {
            Value::Number(n) => Ok(n),
            other => Err(wrong_type("number", &other)),
        }
    }
}

impl<'e> Output<'e> for bool {
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self> {
        e.eval_bool(cx)
    }
    fn from_value(value: Value) -> EvalResult<Self> {
        match value {
            Value::Bool(b) => Ok(b),
            other => Err(wrong_type("boolean", &other)),
        }
    }
}

impl<'e> Output<'e> for Cow<'e, str> {
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self> {
        e.eval_str(cx)
    }
    fn from_value(value: Value) -> EvalResult<Self> {
        match value {
            Value::String(s) => Ok(Cow::Owned(s)),
            other => Err(wrong_type("string", &other)),
        }
    }
}

impl<'e> Output<'e> for Color {
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self> {
        e.eval_color(cx)
    }
    fn from_value(value: Value) -> EvalResult<Self> {
        match value {
            Value::Color(c) => Ok(c),
            other => Err(wrong_type("color", &other)),
        }
    }
}

impl<'e> Output<'e> for Value {
    fn eval(e: &'e Expr, cx: &EvaluationContext<'_>) -> EvalResult<Self> {
        e.evaluate(cx)
    }
    fn from_value(value: Value) -> EvalResult<Self> {
        Ok(value)
    }
}

/// Evaluates `e` for a typed caller whose node has no typed path of its own,
/// through its value.
fn via_value<'e, T: Output<'e>>(e: &Expr, cx: &EvaluationContext<'_>) -> EvalResult<T> {
    T::from_value(e.evaluate(cx)?)
}

impl Expr {
    pub(crate) fn new(node: Node) -> Self {
        let mut deps = node.own_dependencies();
        node.each_child(&mut |child| deps |= child.deps);
        Expr { node, deps }
    }

    pub(crate) fn node(&self) -> &Node {
        &self.node
    }

    /// A literal of `value`, typed as the value is.
    #[must_use]
    pub fn literal(value: Value) -> Self {
        Expr::new(Node::Literal(Box::new(Literal::new(value))))
    }

    /// Parses `value` as a literal: a JSON scalar, or `["literal", value]`.
    pub fn parse_literal(
        value: &mln_conversion::Convertible<'_>,
        ctx: &mut crate::parsing_context::ParsingContext,
    ) -> crate::parsing_context::ParseResult {
        Literal::parse(value, ctx)
    }

    /// `["all", ...]`.
    #[must_use]
    pub fn all(inputs: Vec<Expr>) -> Self {
        Expr::new(Node::All(inputs.into()))
    }

    /// `["any", ...]`.
    #[must_use]
    pub fn any(inputs: Vec<Expr>) -> Self {
        Expr::new(Node::Any(inputs.into()))
    }

    /// The node's static type.
    #[must_use]
    pub fn get_type(&self) -> Type {
        match &self.node {
            Node::Literal(l) => l.get_type().clone(),
            Node::Compound(c) => c.signature().result.clone(),
            Node::Assertion(a) => a.get_type().clone(),
            Node::Coercion(c) => c.get_type().clone(),
            Node::Comparison(_) | Node::All(_) | Node::Any(_) | Node::In(_) | Node::Within(_) => Type::Boolean,
            Node::At(a) => a.get_type(),
            Node::IndexOf(_) | Node::Length(_) | Node::Distance(_) => Type::Number,
            Node::Slice(s) => s.get_type(),
            Node::Case(c) => c.get_type().clone(),
            Node::Coalesce(c) => c.get_type().clone(),
            Node::Match(m) => m.get_type().clone(),
            Node::Step(s) => s.get_type().clone(),
            Node::Interpolate(i) => i.get_type().clone(),
            Node::Let(l) => l.result().get_type(),
            Node::Var(v) => v.bound_expression().get_type(),
            Node::Collator(_) => Type::Collator,
            Node::Format(_) => Type::Formatted,
            Node::Image(_) => Type::Image,
            Node::NumberFormat(_) => Type::String,
            Node::Semiliteral(s) => s.get_type(),
            Node::FormatSectionOverride(_) => Type::Color,
        }
    }

    /// The inputs the expression depends on.
    #[must_use]
    pub fn dependencies(&self) -> Dependency {
        self.deps
    }

    /// Whether the expression depends on any of the inputs in `dep`.
    #[must_use]
    pub fn has(&self, dep: Dependency) -> bool {
        self.deps.intersects(dep)
    }

    /// Visits the node's operands, in the order the style specification
    /// writes them.
    pub fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        self.node.each_child(visit);
    }

    /// The value of a literal, which callers can borrow; `None` for any
    /// other node.
    #[must_use]
    pub fn literal_value(&self) -> Option<&Value> {
        match &self.node {
            Node::Literal(l) => Some(l.get_value()),
            _ => None,
        }
    }

    /// The expression's name in the style specification.
    #[must_use]
    pub fn get_operator(&self) -> &str {
        match &self.node {
            Node::Literal(_) => "literal",
            Node::Compound(c) => c.signature().name,
            Node::Assertion(a) => a.get_operator(),
            Node::Coercion(c) => c.get_operator(),
            Node::Comparison(c) => c.get_operator(),
            Node::All(_) => "all",
            Node::Any(_) => "any",
            Node::At(_) => "at",
            Node::In(_) => "in",
            Node::IndexOf(_) => "index-of",
            Node::Slice(_) => "slice",
            Node::Length(_) => "length",
            Node::Case(_) => "case",
            Node::Coalesce(_) => "coalesce",
            Node::Match(_) => "match",
            Node::Step(_) => "step",
            Node::Interpolate(i) => i.get_operator(),
            Node::Let(_) => "let",
            Node::Var(_) => "var",
            Node::Collator(_) => "collator",
            Node::Format(_) => "format",
            Node::Image(_) => "image",
            Node::NumberFormat(_) => "number-format",
            Node::Within(_) => "within",
            Node::Distance(_) => "distance",
            Node::Semiliteral(_) => "semiliteral",
            Node::FormatSectionOverride(_) => "format-section-override",
        }
    }

    /// The expression as the style specification's JSON, which parses back
    /// to an expression that evaluates alike.
    #[must_use]
    pub fn serialize(&self) -> MbglValue {
        match &self.node {
            Node::Literal(l) => l.serialize(),
            Node::Compound(c) => c.serialize(),
            Node::Assertion(a) => a.serialize(),
            Node::Coercion(c) => c.serialize(),
            Node::Comparison(c) => c.serialize(),
            Node::Interpolate(i) => i.serialize(),
            Node::Step(s) => s.serialize(),
            Node::Match(m) => m.serialize(),
            Node::Let(l) => l.serialize(),
            Node::Var(v) => v.serialize(),
            Node::Collator(c) => c.serialize(),
            Node::Format(f) => f.serialize(),
            Node::Image(i) => i.serialize(),
            Node::NumberFormat(n) => n.serialize(),
            Node::Within(w) => w.serialize(),
            Node::Distance(d) => d.serialize(),
            Node::Semiliteral(s) => s.serialize(),
            Node::FormatSectionOverride(o) => o.serialize(),
            Node::All(_)
            | Node::Any(_)
            | Node::At(_)
            | Node::In(_)
            | Node::Length(_)
            | Node::Case(_)
            | Node::Coalesce(_)
            | Node::IndexOf(_)
            | Node::Slice(_) => self.default_serialize(),
        }
    }

    /// The operator followed by each operand, serialized.
    pub(crate) fn default_serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator().to_owned())];
        self.each_child(&mut |child| serialized.push(child.serialize()));
        MbglValue::Array(serialized)
    }

    /// Statically analyzes the expression, enumerating its possible outputs.
    /// A `None` entry means the complete set is statically undecidable.
    #[must_use]
    pub fn possible_outputs(&self) -> Vec<Option<Value>> {
        match &self.node {
            Node::Literal(l) => vec![Some(l.get_value().clone())],
            Node::Compound(c) => {
                if c.signature().op == Op::Error {
                    Vec::new()
                } else {
                    vec![None]
                }
            }
            Node::All(_) | Node::Any(_) | Node::In(_) | Node::Comparison(_) | Node::Within(_) => {
                vec![Some(Value::Bool(true)), Some(Value::Bool(false))]
            }
            Node::Assertion(_)
            | Node::Coercion(_)
            | Node::Case(_)
            | Node::Coalesce(_)
            | Node::Match(_)
            | Node::Step(_) => {
                let mut result = Vec::new();
                match &self.node {
                    Node::Case(c) => c.each_output(&mut |e| result.extend(e.possible_outputs())),
                    Node::Match(m) => m.each_output(&mut |e| result.extend(e.possible_outputs())),
                    Node::Step(s) => s.each_output(&mut |e| result.extend(e.possible_outputs())),
                    _ => self.each_child(&mut |e| result.extend(e.possible_outputs())),
                }
                result
            }
            Node::Let(l) => l.result().possible_outputs(),
            _ => vec![None],
        }
    }

    /// Feeds `state` the expression's structure, so that equal expressions
    /// hash alike.
    pub fn hash_structure(&self, state: &mut dyn Hasher) {
        let mut state = state;
        discriminant(&self.node).hash(&mut state);
        match &self.node {
            Node::Literal(l) => hash_value(l.get_value(), state),
            Node::Compound(c) => c.signature().name.hash(&mut state),
            Node::Comparison(c) => c.get_operator().hash(&mut state),
            Node::Assertion(_) | Node::Coercion(_) => self.get_type().hash(&mut state),
            Node::Var(v) => v.name().hash(&mut state),
            _ => {}
        }
        self.each_child(&mut |child| child.hash_structure(state));
    }

    /// The node as a compound expression, if it is one.
    pub(crate) fn as_compound(&self) -> Option<&CompoundExpression> {
        match &self.node {
            Node::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// The operand and the literal of `["==", operand, literal]` (or with
    /// the literal first), when the expression is such an equality: it holds
    /// for a feature exactly when the operand evaluates to a value equal to
    /// the literal.
    #[must_use]
    pub fn as_equality(&self) -> Option<(&Expr, &Value)> {
        match &self.node {
            Node::Comparison(c) => c.as_equality(),
            _ => None,
        }
    }

    /// The node as an `interpolate` expression, if it is one.
    #[must_use]
    pub fn as_interpolate(&self) -> Option<&Interpolate> {
        match &self.node {
            Node::Interpolate(i) => Some(i),
            _ => None,
        }
    }

    /// The node as a `format` expression, if it is one.
    #[must_use]
    pub fn as_format(&self) -> Option<&FormatExpression> {
        match &self.node {
            Node::Format(f) => Some(f),
            _ => None,
        }
    }

    /// The node as a `format-section-override`, if it is one.
    #[must_use]
    pub fn as_format_section_override(&self) -> Option<&FormatSectionOverride> {
        match &self.node {
            Node::FormatSectionOverride(o) => Some(o),
            _ => None,
        }
    }

    /// Evaluates the expression to a value of its type.
    pub fn evaluate(&self, cx: &EvaluationContext<'_>) -> EvaluationResult {
        match &self.node {
            Node::Literal(l) => Ok(l.get_value().clone()),
            Node::Compound(c) => c.evaluate(cx),
            Node::Assertion(a) => a.evaluate(cx),
            Node::Coercion(c) => c.evaluate(cx),
            Node::Comparison(c) => c.eval_bool(cx).map(Value::Bool),
            Node::All(_) | Node::Any(_) | Node::In(_) | Node::Within(_) => self.eval_bool(cx).map(Value::Bool),
            Node::IndexOf(_) | Node::Length(_) | Node::Distance(_) => self.eval_num(cx).map(Value::Number),
            Node::NumberFormat(_) => self.eval_str(cx).map(|s| Value::String(s.into_owned())),
            Node::FormatSectionOverride(_) => self.eval_color(cx).map(Value::Color),
            Node::At(a) => a.evaluate(cx),
            Node::Slice(s) => s.evaluate(cx),
            Node::Case(c) => c.eval::<Value>(cx),
            Node::Coalesce(c) => c.evaluate(cx),
            Node::Match(m) => m.eval::<Value>(cx),
            Node::Step(s) => s.eval::<Value>(cx),
            Node::Interpolate(i) => i.evaluate(cx),
            Node::Let(l) => l.result().evaluate(cx),
            Node::Var(v) => v.bound_expression().evaluate(cx),
            Node::Collator(c) => c.evaluate(cx),
            Node::Format(f) => f.evaluate(cx),
            Node::Image(i) => i.evaluate(cx),
            Node::Semiliteral(s) => s.evaluate(cx),
        }
    }

    /// Evaluates through the typed evaluation of the expression's type
    /// ([`eval_num`](Self::eval_num), [`eval_bool`](Self::eval_bool), ...),
    /// giving its result as a value, or through [`evaluate`](Self::evaluate)
    /// for the other types. Wherever `evaluate` gives a value of the type,
    /// this gives the same value.
    pub fn evaluate_typed(&self, cx: &EvaluationContext<'_>) -> EvaluationResult {
        match self.get_type() {
            Type::Number => self.eval_num(cx).map(Value::Number),
            Type::Boolean => self.eval_bool(cx).map(Value::Bool),
            Type::String => self.eval_str(cx).map(|s| Value::String(s.into_owned())),
            Type::Color => self.eval_color(cx).map(Value::Color),
            _ => self.evaluate(cx),
        }
    }

    /// [`evaluate`](Self::evaluate), borrowing a literal's value instead of
    /// cloning it.
    pub(crate) fn evaluate_ref<'e>(&'e self, cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, Value>> {
        match &self.node {
            Node::Literal(l) => Ok(Cow::Borrowed(l.get_value())),
            _ => self.evaluate(cx).map(Cow::Owned),
        }
    }

    /// Evaluates a number-typed expression.
    pub fn eval_num(&self, cx: &EvaluationContext<'_>) -> EvalResult<f64> {
        match &self.node {
            Node::Literal(l) => match l.get_value() {
                Value::Number(n) => Ok(*n),
                other => f64::from_value(other.clone()),
            },
            Node::Compound(c) => c.eval_num(cx),
            Node::Assertion(a) => a.eval_num(cx),
            Node::Coercion(c) => c.eval_num(cx),
            Node::Case(c) => c.eval(cx),
            Node::Match(m) => m.eval(cx),
            Node::Step(s) => s.eval(cx),
            Node::Coalesce(c) => c.eval_typed(cx, &Type::Number),
            Node::Interpolate(i) => i.eval_num(cx),
            Node::Let(l) => l.result().eval_num(cx),
            Node::Var(v) => v.bound_expression().eval_num(cx),
            Node::Length(input) => crate::length::evaluate(input, cx),
            Node::IndexOf(i) => i.eval_num(cx),
            Node::Distance(d) => d.eval_num(cx),
            _ => via_value(self, cx),
        }
    }

    /// Evaluates a boolean-typed expression, such as a filter.
    pub fn eval_bool(&self, cx: &EvaluationContext<'_>) -> EvalResult<bool> {
        match &self.node {
            Node::Literal(l) => match l.get_value() {
                Value::Bool(b) => Ok(*b),
                other => bool::from_value(other.clone()),
            },
            Node::Compound(c) => c.eval_bool(cx),
            Node::Comparison(c) => c.eval_bool(cx),
            Node::All(inputs) => {
                for input in inputs {
                    if !input.eval_bool(cx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Node::Any(inputs) => {
                for input in inputs {
                    if input.eval_bool(cx)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Node::Assertion(a) => a.eval_bool(cx),
            Node::Coercion(c) => c.eval_bool(cx),
            Node::Case(c) => c.eval(cx),
            Node::Match(m) => m.eval(cx),
            Node::Step(s) => s.eval(cx),
            Node::Coalesce(c) => c.eval_typed(cx, &Type::Boolean),
            Node::Let(l) => l.result().eval_bool(cx),
            Node::Var(v) => v.bound_expression().eval_bool(cx),
            Node::In(i) => i.eval_bool(cx),
            Node::Within(w) => Ok(w.eval_bool(cx)),
            _ => via_value(self, cx),
        }
    }

    /// Evaluates a string-typed expression, borrowing a literal's string.
    pub fn eval_str<'e>(&'e self, cx: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
        match &self.node {
            Node::Literal(l) => match l.get_value() {
                Value::String(s) => Ok(Cow::Borrowed(s)),
                other => Cow::from_value(other.clone()),
            },
            Node::Compound(c) => c.eval_str(cx),
            Node::Assertion(a) => a.eval_str(cx),
            Node::Coercion(c) => c.eval_str(cx),
            Node::Case(c) => c.eval(cx),
            Node::Match(m) => m.eval(cx),
            Node::Step(s) => s.eval(cx),
            Node::Coalesce(c) => c.eval_typed(cx, &Type::String),
            Node::Let(l) => l.result().eval_str(cx),
            Node::Var(v) => v.bound_expression().eval_str(cx),
            Node::NumberFormat(n) => n.eval_str(cx).map(Cow::Owned),
            _ => via_value(self, cx),
        }
    }

    /// Evaluates a color-typed expression.
    pub fn eval_color(&self, cx: &EvaluationContext<'_>) -> EvalResult<Color> {
        match &self.node {
            Node::Literal(l) => match l.get_value() {
                Value::Color(c) => Ok(*c),
                other => Color::from_value(other.clone()),
            },
            Node::Compound(c) => c.eval_color(cx),
            Node::Coercion(c) => c.eval_color(cx),
            Node::Case(c) => c.eval(cx),
            Node::Match(m) => m.eval(cx),
            Node::Step(s) => s.eval(cx),
            Node::Coalesce(c) => c.eval_typed(cx, &Type::Color),
            Node::Interpolate(i) => i.eval_color(cx),
            Node::Let(l) => l.result().eval_color(cx),
            Node::Var(v) => v.bound_expression().eval_color(cx),
            Node::FormatSectionOverride(o) => o.eval_color(cx),
            _ => via_value(self, cx),
        }
    }
}

impl Node {
    /// The inputs the node itself reads, apart from its operands'.
    fn own_dependencies(&self) -> Dependency {
        match self {
            Node::Compound(c) => c.signature().dependencies,
            Node::Let(_) => Dependency::BIND,
            Node::Var(v) => Dependency::VAR | v.bound_expression().deps,
            Node::Collator(_) | Node::Within(_) | Node::Distance(_) => Dependency::FEATURE,
            Node::Image(_) => Dependency::IMAGE,
            Node::FormatSectionOverride(_) => Dependency::OVERRIDE | Dependency::FEATURE,
            _ => Dependency::NONE,
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        match self {
            Node::Literal(_) | Node::Var(_) | Node::Within(_) | Node::Distance(_) => {}
            Node::FormatSectionOverride(o) => o.each_child(visit),
            Node::Compound(c) => c.args().iter().for_each(visit),
            Node::Assertion(a) => a.inputs().iter().for_each(visit),
            Node::Coercion(c) => c.inputs().iter().for_each(visit),
            Node::Comparison(c) => c.each_child(visit),
            Node::All(inputs) | Node::Any(inputs) => inputs.iter().for_each(visit),
            Node::At(a) => a.each_child(visit),
            Node::In(i) => i.each_child(visit),
            Node::IndexOf(i) => i.each_child(visit),
            Node::Slice(s) => s.each_child(visit),
            Node::Length(input) => visit(input),
            Node::Case(c) => c.each_child(visit),
            Node::Coalesce(c) => c.args().iter().for_each(visit),
            Node::Match(m) => m.each_child(visit),
            Node::Step(s) => s.each_child(visit),
            Node::Interpolate(i) => i.each_child(visit),
            Node::Let(l) => l.each_child(visit),
            Node::Collator(c) => c.each_child(visit),
            Node::Format(f) => f.each_child(visit),
            Node::Image(i) => visit(i.image_id()),
            Node::NumberFormat(n) => n.each_child(visit),
            Node::Semiliteral(s) => s.elements().iter().for_each(visit),
        }
    }
}

/// Collect the names of the global-state properties referenced by
/// "global-state" expressions in the given expression tree.
pub fn collect_global_state_refs(expression: &Expr, refs: &mut VecSet<String>) {
    if !expression.has(Dependency::GLOBAL_STATE) {
        return;
    }
    if let Some(compound) = expression.as_compound()
        && compound.signature().op == Op::GlobalState
    {
        // The property name argument is enforced to be a string literal
        // at parse time.
        if let Some(Value::String(s)) = compound.args().first().and_then(Expr::literal_value) {
            refs.insert(s.clone());
        }
        return;
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

/// A `GeometryTileFeature` view of a GeoJSON feature.
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
    e: &Expr,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
) -> EvaluationResult {
    let f = GeoJSONFeature::new(feature);
    e.evaluate(&EvaluationContext::with_color_ramp(zoom, Some(&f), color_ramp_parameter))
}

pub fn evaluate_feature_with_images(
    e: &Expr,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
    available_images: &VecSet<String>,
) -> EvaluationResult {
    evaluate_feature_with_context(
        e,
        feature,
        EvaluationContext::with_color_ramp(zoom, None, color_ramp_parameter)
            .with_available_images(Some(available_images)),
    )
}

pub fn evaluate_feature_with_canonical(
    e: &Expr,
    zoom: Option<f32>,
    feature: &Feature<f64>,
    color_ramp_parameter: Option<f64>,
    available_images: &VecSet<String>,
    canonical: &CanonicalTileID,
) -> EvaluationResult {
    evaluate_feature_with_context(
        e,
        feature,
        EvaluationContext::with_color_ramp(zoom, None, color_ramp_parameter)
            .with_available_images(Some(available_images))
            .with_canonical_tile_id(Some(canonical)),
    )
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

/// A bound expression shared by a `let`'s scope and its `var`s.
pub(crate) type SharedExpr = Arc<Expr>;
