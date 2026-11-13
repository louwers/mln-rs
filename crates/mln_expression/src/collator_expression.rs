use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;

use crate::collator::Collator;
use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::literal::Literal;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

pub(crate) struct CollatorExpression {
    base: ExpressionBase,
    case_sensitive: Box<dyn Expression>,
    diacritic_sensitive: Box<dyn Expression>,
    locale: Option<Box<dyn Expression>>,
}

impl CollatorExpression {
    #[must_use]
    pub(crate) fn new(
        case_sensitive: Box<dyn Expression>,
        diacritic_sensitive: Box<dyn Expression>,
        locale: Option<Box<dyn Expression>>,
    ) -> Self {
        // Collators are considered feature-dependent, see `isFeatureConstant`
        let deps = case_sensitive.dependencies()
            | diacritic_sensitive.dependencies()
            | locale.as_ref().map_or(Dependency::NONE, |l| l.dependencies())
            | Dependency::FEATURE;
        CollatorExpression {
            base: ExpressionBase::new(Kind::CollatorExpression, Type::Collator, deps),
            case_sensitive,
            diacritic_sensitive,
            locale,
        }
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
            Some(option) => ctx.parse(&option, 1, Some(Type::Boolean), None),
            None => Some(Box::new(Literal::new(Value::Bool(false))) as Box<dyn Expression>),
        }?;

        let diacritic_sensitive = match options.object_member("diacritic-sensitive") {
            Some(option) => ctx.parse(&option, 1, Some(Type::Boolean), None),
            None => Some(Box::new(Literal::new(Value::Bool(false))) as Box<dyn Expression>),
        }?;

        let mut locale = None;
        if let Some(option) = options.object_member("locale") {
            locale = Some(ctx.parse(&option, 1, Some(Type::String), None)?);
        }

        Some(Box::new(CollatorExpression::new(case_sensitive, diacritic_sensitive, locale)))
    }
}

impl Expression for CollatorExpression {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
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

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.case_sensitive);
        visit(&*self.diacritic_sensitive);
        if let Some(locale) = &self.locale {
            visit(&**locale);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::CollatorExpression {
            let rhs = downcast::<CollatorExpression>(e).expect("CollatorExpression");
            match (&self.locale, &rhs.locale) {
                (None, Some(_)) | (Some(_), None) => return false,
                (Some(l), Some(r)) if !l.equals(&**r) => return false,
                _ => {}
            }
            return self.case_sensitive.equals(&*rhs.case_sensitive)
                && self.diacritic_sensitive.equals(&*rhs.diacritic_sensitive);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        // Technically the set of possible outputs is the combinatoric set of
        // Collators produced by all possibleOutputs of
        // locale/caseSensitive/diacriticSensitive But for the primary use of
        // Collators in comparison operators, we ignore the Collator's
        // possibleOutputs anyway, so we can get away with leaving this
        // undefined for now.
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        let mut options = ValueObject::new();
        options.insert("case-sensitive".to_owned(), self.case_sensitive.serialize());
        options.insert("diacritic-sensitive".to_owned(), self.diacritic_sensitive.serialize());
        if let Some(locale) = &self.locale {
            options.insert("locale".to_owned(), locale.serialize());
        }
        MbglValue::Array(vec![MbglValue::String("collator".to_owned()), MbglValue::Object(options)])
    }

    fn get_operator(&self) -> String {
        "collator".to_owned()
    }
}
