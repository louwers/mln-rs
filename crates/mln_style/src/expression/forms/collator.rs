use std::sync::OnceLock;

use crate::conversion::Convertible;
use crate::expression::locale::collator::Collator;
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::Type;
use crate::expression::value::Value;
use crate::expression::{EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::feature::{Value as MbglValue, ValueObject};

/// `["collator", options]`. Collators count as feature-dependent, so the
/// parser never folds one into a literal; one whose options are literals is
/// made on its first evaluation and kept.
#[derive(Clone, mln_wire::Wire)]
pub(crate) struct CollatorExpression {
    case_sensitive: Expr,
    diacritic_sensitive: Expr,
    locale: Option<Expr>,
    /// The collator of literal options, once made. It crosses as whether the
    /// options are literals.
    #[wire(with = "constant_cache")]
    constant: Option<OnceLock<EvaluationResult>>,
}

mod constant_cache {
    use std::sync::OnceLock;

    use crate::expression::EvaluationResult;

    #[expect(clippy::ref_option, reason = "`#[wire(with)]` passes the field by reference")]
    pub(super) fn write(cache: &Option<OnceLock<EvaluationResult>>, w: &mut mln_wire::Writer) {
        mln_wire::Wire::write(&cache.is_some(), w);
    }

    #[expect(clippy::option_option, reason = "a read that can fail, of an `Option`")]
    pub(super) fn read(r: &mut mln_wire::Reader<'_>) -> Option<Option<OnceLock<EvaluationResult>>> {
        Some(<bool as mln_wire::Wire>::read(r)?.then(OnceLock::new))
    }
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

        let boolean =
            |value: &Value| value.as_bool().ok_or_else(|| EvaluationError::type_mismatch(&Type::Boolean, value));
        let locale = evaluated_locale
            .as_ref()
            .map(|locale| locale.as_str().ok_or_else(|| EvaluationError::type_mismatch(&Type::String, locale)))
            .transpose()?;
        Collator::new(boolean(&case_sensitive_result)?, boolean(&diacritic_sensitive_result)?, locale)
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
