use std::sync::OnceLock;

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;

use crate::collator::Collator;
use crate::expression::{EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

/// `["collator", options]`. Collators count as feature-dependent, so the
/// parser never folds one into a literal; one whose options are literals is
/// made on its first evaluation and kept.
#[derive(Clone)]
pub(crate) struct CollatorExpression {
    case_sensitive: Expr,
    diacritic_sensitive: Expr,
    locale: Option<Expr>,
    /// The collator of literal options, once made.
    constant: Option<OnceLock<EvaluationResult>>,
}

impl PartialEq for CollatorExpression {
    fn eq(&self, other: &Self) -> bool {
        self.case_sensitive == other.case_sensitive
            && self.diacritic_sensitive == other.diacritic_sensitive
            && self.locale == other.locale
    }
}

impl CollatorExpression {
    #[cfg(test)]
    pub(crate) fn new_expr(case_sensitive: Expr, diacritic_sensitive: Expr, locale: Option<Expr>) -> Expr {
        Expr::new(Node::Collator(Box::new(CollatorExpression {
            case_sensitive,
            diacritic_sensitive,
            locale,
            constant: None,
        })))
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        if value.array_length() != 2 {
            ctx.error("Expected one argument.".to_owned());
            return None;
        }

        let options = value.array_member(1);
        if !options.is_object() {
            ctx.error("Collator options argument must be an object.".to_owned());
            return None;
        }

        let case_sensitive = match options.object_member("case-sensitive") {
            Some(option) => ctx.parse(&option, 1, Some(Type::Boolean), None)?,
            None => Expr::literal(Value::Bool(false)),
        };

        let diacritic_sensitive = match options.object_member("diacritic-sensitive") {
            Some(option) => ctx.parse(&option, 1, Some(Type::Boolean), None)?,
            None => Expr::literal(Value::Bool(false)),
        };

        let mut locale = None;
        if let Some(option) = options.object_member("locale") {
            locale = Some(ctx.parse(&option, 1, Some(Type::String), None)?);
        }

        let constant = case_sensitive.literal_value().is_some()
            && diacritic_sensitive.literal_value().is_some()
            && locale.as_ref().is_none_or(|locale| locale.literal_value().is_some());
        Some(Expr::new(Node::Collator(Box::new(CollatorExpression {
            case_sensitive,
            diacritic_sensitive,
            locale,
            constant: constant.then(OnceLock::new),
        }))))
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        match &self.constant {
            Some(constant) => constant.get_or_init(|| self.make(params)).clone(),
            None => self.make(params),
        }
    }

    fn make(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let case_sensitive_result = self.case_sensitive.evaluate(params)?;
        let diacritic_sensitive_result = self.diacritic_sensitive.evaluate(params)?;

        let mut evaluated_locale = None;
        if let Some(locale) = &self.locale {
            evaluated_locale = Some(locale.evaluate(params)?);
        }

        Collator::new(
            case_sensitive_result.as_bool().expect("boolean option"),
            diacritic_sensitive_result.as_bool().expect("boolean option"),
            evaluated_locale.as_ref().map(|locale| locale.as_str().expect("string locale")),
        )
        .map(Value::Collator)
        .map_err(EvaluationError::new)
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.case_sensitive);
        visit(&self.diacritic_sensitive);
        if let Some(locale) = &self.locale {
            visit(locale);
        }
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut options = ValueObject::new();
        options.insert("case-sensitive".to_owned(), self.case_sensitive.serialize());
        options.insert("diacritic-sensitive".to_owned(), self.diacritic_sensitive.serialize());
        if let Some(locale) = &self.locale {
            options.insert("locale".to_owned(), locale.serialize());
        }
        MbglValue::Array(vec![MbglValue::String("collator".to_owned()), MbglValue::Object(options)])
    }
}
