// Ports include/mln/style/expression/parsing_context.hpp and
// src/mln/style/expression/parsing_context.cpp.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_conversion::{Convertible, get_json_type};
use mln_vec_map::VecMap;

use crate::assertion::Assertion;
use crate::at::At;
use crate::boolean_operator::{All, Any};
use crate::case::Case;
use crate::check_subtype::check_subtype;
use crate::coalesce::Coalesce;
use crate::coercion::Coercion;
use crate::collator_expression::CollatorExpression;
use crate::comparison::parse_comparison;
use crate::compound_expression::{CompoundExpression, parse_compound_expression};
use crate::distance::Distance;
use crate::expression::{EvaluationContext, Expression, Kind, downcast};
use crate::find_zoom_curve::{ZoomCurve, find_zoom_curve};
use crate::format_expression::FormatExpression;
use crate::image_expression::ImageExpression;
use crate::in_::In;
use crate::index_of::IndexOf;
use crate::interpolate::parse_interpolate;
use crate::is_constant::{is_feature_constant, is_global_property_constant, is_zoom_constant};
use crate::length::Length;
use crate::let_::{Let, Var};
use crate::literal::Literal;
use crate::match_::parse_match;
use crate::number_format::NumberFormat;
use crate::semiliteral::Semiliteral;
use crate::slice::Slice;
use crate::step::Step;
use crate::type_::Type;
use crate::value::Value;
use crate::within::Within;

/// `expression::ParsingError`. `Display` writes `key: message`, or the
/// message alone when the key is empty, as `getCombinedErrors` does.
#[derive(Clone, Debug, Default, PartialEq, Eq, thiserror::Error)]
pub struct ParsingError {
    pub message: String,
    pub key: String,
}

impl std::fmt::Display for ParsingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.key.is_empty() {
            write!(f, "{}: ", self.key)?;
        }
        f.write_str(&self.message)
    }
}

/// `expression::ParseResult` (`std::optional<std::unique_ptr<Expression>>`).
pub type ParseResult = Option<Box<dyn Expression>>;

/// The bindings of a `let` scope (`std::map<std::string, std::shared_ptr<Expression>>`).
pub type Bindings = VecMap<String, Arc<dyn Expression>>;

/// `detail::Scope`.
pub struct Scope {
    bindings: Bindings,
    parent: Option<Rc<Scope>>,
}

impl Scope {
    #[must_use]
    pub fn new(bindings: Bindings, parent: Option<Rc<Scope>>) -> Self {
        Scope { bindings, parent }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Expression>> {
        if let Some(binding) = self.bindings.get(name) {
            Some(binding.clone())
        } else if let Some(parent) = &self.parent {
            parent.get(name)
        } else {
            None
        }
    }
}

/// Controls the annotation behavior of the parser when encountering an
/// expression whose type is not a subtype of the expected type. The default
/// behavior, used when `Option<TypeAnnotationOption>` is `None`, is as
/// follows:
///
/// When we expect a number, string, boolean, or array but have a value, wrap
/// it in an assertion. When we expect a color or formatted string, but have a
/// string or value, wrap it in a coercion. Otherwise, we do static
/// type-checking.
///
/// These behaviors are overridable for:
///   * The "coalesce" operator, which needs to omit type annotations.
///   * String-valued properties (e.g. `text-field`), where coercion is more
///     convenient than assertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeAnnotationOption {
    Coerce,
    Assert,
    Omit,
}

/// `expression::ParsingContext`.
pub struct ParsingContext {
    key: String,
    expected: Option<Type>,
    scope: Option<Rc<Scope>>,
    errors: Rc<RefCell<Vec<ParsingError>>>,
}

impl Default for ParsingContext {
    fn default() -> Self {
        ParsingContext { key: String::new(), expected: None, scope: None, errors: Rc::new(RefCell::new(Vec::new())) }
    }
}

// Port-only: unlike ffi patch 0093, "is-supported-script" isn't listed, so
// `["is-supported-script", "<literal>"]` still folds into a constant, without
// font faces. Left unfolded, a feature-independent use in a layout property
// such as text-field wasn't a literal, and property conversion rejected it.
// The patch's render tests, like styles in practice, pass a feature property,
// which keeps the expression from folding anyway.
const REQUIRED_PROPS: [&str; 6] =
    ["zoom", "heatmap-density", "line-progress", "accumulated", "elevation", "global-state"];

fn is_constant(expression: &dyn Expression) -> bool {
    let kind = expression.get_kind();

    if kind == Kind::Var {
        let var_expression = downcast::<Var>(expression).expect("Var");
        return is_constant(&**var_expression.get_bound_expression());
    } else if kind == Kind::CompoundExpression {
        let compound = downcast::<CompoundExpression>(expression).expect("CompoundExpression");
        if compound.get_operator() == "error" {
            return false;
        }
    }

    let is_type_annotation = kind == Kind::Coercion || kind == Kind::Assertion;

    let mut children_constant = true;
    expression.each_child(&mut |child| {
        // We can _almost_ assume that if `expressions` children are constant,
        // they would already have been evaluated to Literal values when they
        // were parsed. Type annotations are the exception, because they might
        // have been inferred and added after a child was parsed.

        // So we recurse into isConstant() for the children of type
        // annotations, but otherwise simply check whether they are Literals.
        if is_type_annotation {
            children_constant = children_constant && is_constant(child);
        } else {
            children_constant = children_constant && child.get_kind() == Kind::Literal;
        }
    });

    children_constant && is_feature_constant(expression) && is_global_property_constant(expression, &REQUIRED_PROPS)
}

type ParseFunction = fn(&Convertible<'_>, &mut ParsingContext) -> ParseResult;

/// The `expressionRegistry` of `parsing_context.cpp`.
fn expression_registry(name: &str) -> Option<ParseFunction> {
    Some(match name {
        "==" | "!=" | ">" | "<" | ">=" | "<=" => parse_comparison,
        "all" => All::parse,
        "any" => Any::parse,
        "array" | "boolean" | "number" | "object" | "string" => Assertion::parse,
        "at" => At::parse,
        "in" => In::parse,
        "case" => Case::parse,
        "coalesce" => Coalesce::parse,
        "collator" => CollatorExpression::parse,
        "distance" => Distance::parse,
        "format" => FormatExpression::parse,
        "image" => ImageExpression::parse,
        "interpolate" | "interpolate-hcl" | "interpolate-lab" => parse_interpolate,
        "length" => Length::parse,
        "let" => Let::parse,
        "literal" => Literal::parse,
        "match" => parse_match,
        "number-format" => NumberFormat::parse,
        "step" => Step::parse,
        "to-boolean" | "to-color" | "to-padding" | "to-number" | "to-string" => Coercion::parse,
        "var" => Var::parse,
        "within" => Within::parse,
        "index-of" => IndexOf::parse,
        "slice" => Slice::parse,
        "semiliteral" => Semiliteral::parse,
        _ => return None,
    })
}

/// `isExpression(const std::string&)`: whether `name` is in the expression
/// registry (not the compound-expression registry).
#[must_use]
pub fn is_expression_name(name: &str) -> bool {
    expression_registry(name).is_some()
}

impl ParsingContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `ParsingContext(std::string key)`.
    pub fn with_key(key: impl Into<String>) -> Self {
        ParsingContext { key: key.into(), ..Default::default() }
    }

    /// `explicit ParsingContext(type::Type expected)`.
    #[must_use]
    pub fn with_expected(expected: Type) -> Self {
        ParsingContext { expected: Some(expected), ..Default::default() }
    }

    fn child(
        key: String,
        errors: Rc<RefCell<Vec<ParsingError>>>,
        expected: Option<Type>,
        scope: Option<Rc<Scope>>,
    ) -> Self {
        ParsingContext { key, expected, scope, errors }
    }

    #[must_use]
    pub fn get_key(&self) -> String {
        self.key.clone()
    }

    #[must_use]
    pub fn get_expected(&self) -> Option<Type> {
        self.expected.clone()
    }

    #[must_use]
    pub fn get_errors(&self) -> Vec<ParsingError> {
        self.errors.borrow().clone()
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.borrow().is_empty()
    }

    #[must_use]
    pub fn get_combined_errors(&self) -> String {
        self.errors.borrow().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    }

    /// Parse the given style-spec JSON value as an expression.
    pub fn parse_expression(
        &mut self,
        value: &Convertible<'_>,
        type_annotation_option: Option<TypeAnnotationOption>,
    ) -> ParseResult {
        self.parse_value(value, type_annotation_option)
    }

    /// Parse the given style-spec JSON value as an expression intended to be
    /// used in a layout or paint property. This entails checking additional
    /// constraints that exist in that context but not, e.g., for filters.
    pub fn parse_layer_property_expression(&mut self, value: &Convertible<'_>) -> ParseResult {
        let type_annotation_option =
            if self.expected.as_ref() == Some(&Type::String) { Some(TypeAnnotationOption::Coerce) } else { None };
        let parsed = self.parse_value(value, type_annotation_option);

        // If the expression is zoom-dependent, validate that we can resolve a
        // zoom curve or treat it as an error
        if let Some(p) = &parsed {
            debug_assert_eq!(!p.has(crate::expression::Dependency::ZOOM), is_zoom_constant(&**p));
            if p.has(crate::expression::Dependency::ZOOM) {
                match find_zoom_curve(&**p) {
                    Some(ZoomCurve::Error(err)) => {
                        self.error(err.message);
                        return None;
                    }
                    Some(_) => {}
                    None => {
                        self.error(
                            r#""zoom" expression may only be used as input to a top-level "step" or "interpolate" expression."#
                                .to_owned(),
                        );
                        return None;
                    }
                }
            }
        }
        parsed
    }

    /// Parse a child expression. For use by individual `Expression::parse()`
    /// methods.
    pub fn parse(
        &mut self,
        value: &Convertible<'_>,
        index: usize,
        expected: Option<Type>,
        type_annotation_option: Option<TypeAnnotationOption>,
    ) -> ParseResult {
        let mut child = ParsingContext::child(
            format!("{}[{}]", self.key, index),
            self.errors.clone(),
            expected,
            self.scope.clone(),
        );
        child.parse_value(value, type_annotation_option)
    }

    /// Parse a child expression with additional bindings in scope.
    pub fn parse_with_bindings(
        &mut self,
        value: &Convertible<'_>,
        index: usize,
        expected: Option<Type>,
        bindings: &Bindings,
    ) -> ParseResult {
        let mut child = ParsingContext::child(
            format!("{}[{}]", self.key, index),
            self.errors.clone(),
            expected,
            Some(Rc::new(Scope::new(bindings.clone(), self.scope.clone()))),
        );
        child.parse_value(value, None)
    }

    /// Check whether `t` is a subtype of `expected`, collecting an error if
    /// not.
    pub fn check_type(&mut self, t: &Type) -> Option<String> {
        let expected = self.expected.as_ref().expect("check_type without an expected type");
        let err = check_subtype(expected, t);
        if let Some(err) = &err {
            self.error(err.clone());
        }
        err
    }

    #[must_use]
    pub fn get_binding(&self, name: &str) -> Option<Arc<dyn Expression>> {
        self.scope.as_ref()?.get(name)
    }

    pub fn error(&mut self, message: String) {
        self.errors.borrow_mut().push(ParsingError { message, key: self.key.clone() });
    }

    pub fn error_at(&mut self, message: String, child: usize) {
        self.errors.borrow_mut().push(ParsingError { message, key: format!("{}[{}]", self.key, child) });
    }

    pub fn error_at2(&mut self, message: String, child: usize, grandchild: usize) {
        self.errors
            .borrow_mut()
            .push(ParsingError { message, key: format!("{}[{}][{}]", self.key, child, grandchild) });
    }

    pub fn append_errors(&mut self, ctx: &ParsingContext) {
        let mut theirs = ctx.errors.borrow_mut();
        self.errors.borrow_mut().append(&mut theirs);
    }

    pub fn clear_errors(&mut self) {
        self.errors.borrow_mut().clear();
    }

    /// Parse the given style-spec JSON value into an Expression object.
    /// Specifically, this function is responsible for determining the
    /// expression type (either Literal, or the one named in value[0]) and
    /// dispatching to the appropriate `ParseXxxx::parse(const` V&,
    /// `ParsingContext`) method.
    fn parse_value(
        &mut self,
        value: &Convertible<'_>,
        type_annotation_option: Option<TypeAnnotationOption>,
    ) -> ParseResult {
        let mut parsed: ParseResult;

        if value.is_array() {
            let length = value.array_length();
            if length == 0 {
                self.error(
                    r#"Expected an array with at least one element. If you wanted a literal array, use ["literal", []]."#
                        .to_owned(),
                );
                return None;
            }

            let Some(op) = value.array_member(0).to_string() else {
                self.error_at(
                    format!(
                        r#"Expression name must be a string, but found {} instead. If you wanted a literal array, use ["literal", [...]]."#,
                        get_json_type(&value.array_member(0))
                    ),
                    0,
                );
                return None;
            };

            parsed = if let Some(parse_function) = expression_registry(&op) {
                parse_function(value, self)
            } else {
                parse_compound_expression(&op, value, self)
            };
        } else {
            parsed = Literal::parse(value, self);
        }

        let Some(mut expr) = parsed else {
            debug_assert!(!self.errors.borrow().is_empty());
            return None;
        };

        let annotate = |expression: Box<dyn Expression>,
                        type_: Type,
                        type_annotation: TypeAnnotationOption|
         -> Box<dyn Expression> {
            match type_annotation {
                TypeAnnotationOption::Assert => Box::new(Assertion::new(type_, vec![expression])),
                TypeAnnotationOption::Coerce => Box::new(Coercion::new(type_, vec![expression])),
                TypeAnnotationOption::Omit => expression,
            }
        };

        if let Some(expected) = self.expected.clone() {
            let actual = expr.get_type().clone();
            if (expected == Type::String
                || expected == Type::Number
                || expected == Type::Boolean
                || expected == Type::Object
                || expected.is_array())
                && actual == Type::Value
            {
                expr = annotate(expr, expected, type_annotation_option.unwrap_or(TypeAnnotationOption::Assert));
            } else if ((expected == Type::Color || expected == Type::Formatted || expected == Type::Image)
                && (actual == Type::Value || actual == Type::String))
                || (expected == Type::Padding && (actual == Type::Value || actual == Type::Number || actual.is_array()))
                || (expected == Type::ProjectionDefinition
                    && (actual == Type::Value || actual == Type::String || actual.is_array()))
                || (expected == Type::VariableAnchorOffsetCollection && (actual == Type::Value || actual.is_array()))
            {
                expr = annotate(expr, expected, type_annotation_option.unwrap_or(TypeAnnotationOption::Coerce));
            } else {
                let t = expr.get_type().clone();
                self.check_type(&t);
                if !self.errors.borrow().is_empty() {
                    return None;
                }
            }
        }
        parsed = Some(expr);
        let expr = parsed.as_ref().unwrap();

        // If an expression's arguments are all constant, we can evaluate it
        // immediately and replace it with a literal value in the
        // parsed/compiled result. Expressions that expect an image should not
        // be resolved here so we can later get the available images.
        if expr.get_kind() != Kind::Literal && *expr.get_type() != Type::Image && is_constant(&**expr) {
            let params = EvaluationContext::from_feature(None);
            let evaluated = expr.evaluate(&params);
            let evaluated = match evaluated {
                Ok(v) => v,
                Err(e) => {
                    self.error(e.message);
                    return None;
                }
            };

            let type_ = expr.get_type().clone();
            if let Type::Array(array) = type_ {
                // keep the original expression's array type, even if the
                // evaluated type is more specific.
                let Value::Array(items) = evaluated else {
                    panic!("array-typed expression evaluated to a non-array");
                };
                return Some(Box::new(Literal::with_array_type(*array, items)));
            }
            return Some(Box::new(Literal::new(evaluated)));
        }

        parsed
    }
}
