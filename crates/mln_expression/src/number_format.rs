// Ports include/mln/style/expression/number_format.hpp and
// src/mln/style/expression/number_format.cpp. The platform number format is
// the `locale` module's.

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::locale::{self, FractionDigits, currency_code};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, to_string};

const LOCALE_KEY: &str = "locale";
const CURRENCY_KEY: &str = "currency";
const MIN_FRACTION_DIGITS_KEY: &str = "min-fraction-digits";
const MAX_FRACTION_DIGITS_KEY: &str = "max-fraction-digits";

/// `platform::formatNumber`: `number` in a BCP 47 locale, or in the
/// platform's default locale when the locale is empty, malformed or its
/// language unsupported, rounded half to even to at most `max` fraction
/// digits and padded to at least `min`. Omitted limits default to 0 and 3,
/// or to the currency's digits for currency, and an explicit maximum wins
/// over a minimum. With an ISO 4217 currency code, it formats as an amount of
/// that currency, with the locale's pattern and symbol.
///
/// # Errors
///
/// When the currency isn't three ASCII letters, or the platform fails to
/// format.
fn format_number(
    number: f64,
    locale: &str,
    currency: &str,
    min: Option<u8>,
    max: Option<u8>,
) -> Result<String, String> {
    let currency = currency_code(currency)?;
    locale::format_number(number, locale, currency.as_deref(), FractionDigits::new(min, max))
}

pub struct NumberFormat {
    base: ExpressionBase,
    number: Box<dyn Expression>,
    locale: Option<Box<dyn Expression>>,
    currency: Option<Box<dyn Expression>>,
    min_fraction_digits: Option<Box<dyn Expression>>,
    max_fraction_digits: Option<Box<dyn Expression>>,
}

fn opt_deps(e: Option<&dyn Expression>) -> Dependency {
    e.map_or(Dependency::NONE, Expression::dependencies)
}

fn opt_equal(l: Option<&dyn Expression>, r: Option<&dyn Expression>) -> bool {
    match (l, r) {
        (Some(l), Some(r)) => l.equals(r),
        (None, None) => true,
        _ => false,
    }
}

/// `static_cast<uint8_t>(double)`: through int32 on x86-64.
fn static_cast_u8(d: f64) -> u8 {
    (if d > -2_147_483_649.0 && d < 2_147_483_648.0 { d as i32 } else { i32::MIN }) as u8
}

impl NumberFormat {
    #[must_use]
    pub fn new(
        number: Box<dyn Expression>,
        locale: Option<Box<dyn Expression>>,
        currency: Option<Box<dyn Expression>>,
        min_fraction_digits: Option<Box<dyn Expression>>,
        max_fraction_digits: Option<Box<dyn Expression>>,
    ) -> Self {
        let deps = number.dependencies()
            | opt_deps(locale.as_deref())
            | opt_deps(currency.as_deref())
            | opt_deps(min_fraction_digits.as_deref())
            | opt_deps(max_fraction_digits.as_deref());
        NumberFormat {
            base: ExpressionBase::new(Kind::NumberFormat, Type::String, deps),
            number,
            locale,
            currency,
            min_fraction_digits,
            max_fraction_digits,
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let length = value.array_length();

        if length != 3 {
            ctx.error(format!("Expected two arguments, but found {length} instead."));
            return None;
        }

        let Some(number_result) = ctx.parse(&value.array_member(1), 1, Some(Type::Number), None) else {
            ctx.error("Failed to parse the number.".to_owned());
            return None;
        };

        let t = number_result.get_type();
        if *t != Type::Number {
            ctx.error(format!("Expected argument of type number, but found {} instead.", type_::to_string(t)));
            return None;
        }

        let options = value.array_member(2);
        if !options.is_object() {
            ctx.error("Number-format options argument must be an object.".to_owned());
            return None;
        }

        let mut parse_option = |key: &str, t: Type, message: &str| -> Result<Option<Box<dyn Expression>>, ()> {
            if let Some(option) = options.object_member(key) {
                if let Some(r) = ctx.parse(&option, 1, Some(t), None) {
                    Ok(Some(r))
                } else {
                    ctx.error(message.to_owned());
                    Err(())
                }
            } else {
                Ok(None)
            }
        };

        let locale_result = parse_option(LOCALE_KEY, Type::String, "Number-format locale parsing failed.").ok()?;
        let currency_result =
            parse_option(CURRENCY_KEY, Type::String, "Number-format currency parsing failed.").ok()?;
        let min_fraction_digits_result =
            parse_option(MIN_FRACTION_DIGITS_KEY, Type::Number, "Number-format min-fraction-digits parsing failed.")
                .ok()?;
        let max_fraction_digits_result =
            parse_option(MAX_FRACTION_DIGITS_KEY, Type::Number, "Number-format max-fraction-digits parsing failed.")
                .ok()?;

        Some(Box::new(NumberFormat::new(
            number_result,
            locale_result,
            currency_result,
            min_fraction_digits_result,
            max_fraction_digits_result,
        )))
    }
}

impl Expression for NumberFormat {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let number_result = self.number.evaluate(params)?;
        let evaluated_number = number_result.as_number().expect("numeric input");

        let mut evaluated_locale = String::new();
        if let Some(locale) = &self.locale {
            evaluated_locale = to_string(&locale.evaluate(params)?);
        }

        let mut evaluated_currency = String::new();
        if let Some(currency) = &self.currency {
            evaluated_currency = to_string(&currency.evaluate(params)?);
        }

        let mut evaluated_min_fraction_digits = None;
        if let Some(min) = &self.min_fraction_digits {
            evaluated_min_fraction_digits = Some(static_cast_u8(min.evaluate(params)?.as_number().expect("number")));
        }

        let mut evaluated_max_fraction_digits = None;
        if let Some(max) = &self.max_fraction_digits {
            evaluated_max_fraction_digits = Some(static_cast_u8(max.evaluate(params)?.as_number().expect("number")));
        }

        format_number(
            evaluated_number,
            &evaluated_locale,
            &evaluated_currency,
            evaluated_min_fraction_digits,
            evaluated_max_fraction_digits,
        )
        .map(Value::String)
        .map_err(EvaluationError::new)
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.number);
        for e in
            [&self.locale, &self.currency, &self.min_fraction_digits, &self.max_fraction_digits].into_iter().flatten()
        {
            visit(&**e);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::NumberFormat {
            let rhs = downcast::<NumberFormat>(e).expect("NumberFormat");
            return opt_equal(self.locale.as_deref(), rhs.locale.as_deref())
                && opt_equal(self.currency.as_deref(), rhs.currency.as_deref())
                && opt_equal(self.min_fraction_digits.as_deref(), rhs.min_fraction_digits.as_deref())
                && opt_equal(self.max_fraction_digits.as_deref(), rhs.max_fraction_digits.as_deref())
                && self.number.equals(&*rhs.number);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        let mut options = ValueObject::new();
        if let Some(locale) = &self.locale {
            options.insert(LOCALE_KEY.to_owned(), locale.serialize());
        }
        if let Some(currency) = &self.currency {
            options.insert(CURRENCY_KEY.to_owned(), currency.serialize());
        }
        if let Some(min) = &self.min_fraction_digits {
            options.insert(MIN_FRACTION_DIGITS_KEY.to_owned(), min.serialize());
        }
        if let Some(max) = &self.max_fraction_digits {
            options.insert(MAX_FRACTION_DIGITS_KEY.to_owned(), max.serialize());
        }
        MbglValue::Array(vec![
            MbglValue::String(self.get_operator()),
            self.number.serialize(),
            MbglValue::Object(options),
        ])
    }

    fn get_operator(&self) -> String {
        "number-format".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::format_number;

    fn format(number: f64, locale: &str, currency: &str, min: Option<u8>, max: Option<u8>) -> String {
        format_number(number, locale, currency, min, max).expect("formatted number")
    }

    #[test]
    fn decimals_round_half_to_even_within_the_fraction_limits() {
        assert_eq!(format(123_456.789, "en-US", "", None, None), "123,456.789");
        assert_eq!(format(987_654_321.234_567, "en-US", "", Some(15), Some(20)), "987,654,321.234567000000000");
        assert_eq!(format(987_654_321.234_567, "en-US", "", Some(2), Some(4)), "987,654,321.2346");
        assert_eq!(format(12.3, "en-US", "", Some(4), None), "12.3000");
        assert_eq!(format(12.355, "en-US", "", Some(4), Some(2)), "12.36");
        assert_eq!(format(12.345, "en-US", "", None, Some(2)), "12.34");
        assert_eq!(format(0.5, "en-US", "", None, Some(0)), "0");
        assert_eq!(format(1.5, "en-US", "", None, Some(0)), "2");
        assert_eq!(format(0.0005, "en-US", "", None, None), "0");
        assert_eq!(format(-1234.5, "en-US", "", None, None), "-1,234.5");
    }

    #[test]
    fn decimals_follow_the_locale() {
        assert_eq!(format(1234.5, "de-DE", "", None, None), "1.234,5");
        assert_eq!(format(1_234_567.5, "fr-FR", "", None, None), "1\u{202f}234\u{202f}567,5");
        assert_eq!(format(1_234_567.5, "hi-IN", "", None, None), "12,34,567.5");
        assert_eq!(format(1234.5, "ar-EG", "", None, None), "١٬٢٣٤٫٥");
        assert_eq!(format(1234.5, "xx-ZZ", "", None, None), format(1234.5, "", "", None, None));
    }

    #[test]
    fn currencies_take_their_digits_and_the_locale_pattern() {
        assert_eq!(format(123_456.789, "en-US", "JPY", None, None), "¥123,457");
        assert_eq!(format(123_456.789, "en-US", "EUR", None, None), "€123,456.79");
        assert_eq!(format(1234.5, "de-DE", "EUR", None, None), "1.234,50\u{a0}€");
        // CLDR gives Japanese the full-width yen sign; macOS 27's Foundation
        // gives the ordinary one.
        let yen = format(1234.6, "ja-JP", "JPY", None, None);
        if cfg!(target_vendor = "apple") {
            assert!(["¥1,235", "￥1,235"].contains(&yen.as_str()), "{yen}");
        } else {
            assert_eq!(yen, "￥1,235");
        }
        assert_eq!(format(12.3, "en-US", "USD", None, Some(0)), "$12");
        assert_eq!(format(12.3, "en-US", "usd", Some(3), None), "$12.300");
        assert_eq!(format(-12.3, "en-US", "USD", None, None), "-$12.30");
        assert_eq!(format(12.0, "en-US", "XYZ", None, None), "XYZ\u{a0}12.00");
    }

    #[test]
    fn non_finite_numbers_have_no_digits() {
        assert_eq!(format(f64::NAN, "en-US", "", None, None), "NaN");
        assert_eq!(format(f64::INFINITY, "en-US", "", None, None), "∞");
        assert_eq!(format(f64::NEG_INFINITY, "en-US", "", None, None), "-∞");
        // Foundation's currency formats may leave the symbol out of an
        // infinity.
        let currency = format(f64::NEG_INFINITY, "en-US", "USD", None, None);
        if cfg!(target_vendor = "apple") {
            assert!(currency.starts_with('-') && currency.ends_with('∞'), "{currency}");
        } else {
            assert_eq!(currency, "-$∞");
        }
    }

    #[test]
    fn a_currency_that_is_not_an_iso_4217_code_fails() {
        assert!(format_number(1.0, "en-US", "US$", None, None).is_err());
        assert!(format_number(1.0, "en-US", "EURO", None, None).is_err());
    }
}
