// Ports include/mln/style/expression/number_format.hpp and
// src/mln/style/expression/number_format.cpp.

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;

use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, to_string};

const LOCALE_KEY: &str = "locale";
const CURRENCY_KEY: &str = "currency";
const MIN_FRACTION_DIGITS_KEY: &str = "min-fraction-digits";
const MAX_FRACTION_DIGITS_KEY: &str = "max-fraction-digits";

/// `platform::formatNumber` (`platform/default/src/mln/i18n/number_format.cpp`,
/// ICU `NumberFormatter`).
// TODO(port): ICU number formatting. This implements ICU's behaviour for the
// root/en locales only: the shortest round-trip decimal of the double, rounded
// half-even to maxFractionDigits and padded to minFractionDigits, ',' grouping
// and '.' decimal separator; with a currency, ICU's currency precision and an
// en-US symbol for a few common currencies. The locale is ignored.
fn format_number(
    number: f64,
    _locale_id: &str,
    currency: &str,
    min_fraction_digits: u8,
    max_fraction_digits: u8,
) -> String {
    if number.is_nan() {
        return "NaN".to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 { "∞" } else { "-∞" }.to_owned();
    }

    let (symbol, min_fraction, max_fraction) = if currency.is_empty() {
        (None, min_fraction_digits as usize, max_fraction_digits as usize)
    } else {
        let digits = match currency {
            "JPY" | "KRW" | "VND" | "CLP" | "ISK" | "UGX" | "PYG" => 0,
            _ => 2,
        };
        let symbol = match currency {
            "USD" => "$".to_owned(),
            "EUR" => "€".to_owned(),
            "JPY" => "¥".to_owned(),
            "GBP" => "£".to_owned(),
            "CNY" => "CN¥".to_owned(),
            "INR" => "₹".to_owned(),
            "KRW" => "₩".to_owned(),
            "CAD" => "CA$".to_owned(),
            "AUD" => "A$".to_owned(),
            other => format!("{other}\u{a0}"),
        };
        (Some(symbol), digits, digits)
    };
    let max_fraction = max_fraction.max(min_fraction);

    // Shortest round-trip digits of |number| (`digits` is 0.d0d1d2... *
    // 10^point, so the decimal point sits after index `point`).
    let (mut digits, mut point) = shortest_digits(number.abs());
    if digits.iter().all(|d| *d == 0) {
        digits = vec![0];
        point = 1;
    }

    // Round half-even at max_fraction fraction digits.
    let keep = point + max_fraction as i64;
    if keep < digits.len() as i64 {
        let (head_len, round_up) = if keep < 0 {
            (0usize, false)
        } else {
            let keep = keep as usize;
            let first = digits[keep];
            let rest_nonzero = digits[keep + 1..].iter().any(|d| *d != 0);
            let last_kept_odd = keep > 0 && digits[keep - 1] % 2 == 1;
            (keep, first > 5 || (first == 5 && (rest_nonzero || last_kept_odd)))
        };
        digits.truncate(head_len);
        if round_up {
            let mut i = digits.len();
            loop {
                if i == 0 {
                    digits.insert(0, 1);
                    point += 1;
                    break;
                }
                i -= 1;
                if digits[i] == 9 {
                    digits[i] = 0;
                } else {
                    digits[i] += 1;
                    break;
                }
            }
        }
    }

    // Integer and fraction digit strings.
    let digit_at = |i: i64| -> u8 { if i >= 0 && (i as usize) < digits.len() { digits[i as usize] } else { 0 } };
    let mut int_part = String::new();
    if point <= 0 {
        int_part.push('0');
    } else {
        for i in 0..point {
            int_part.push((b'0' + digit_at(i)) as char);
        }
    }
    let mut frac_part = String::new();
    for i in 0..max_fraction as i64 {
        frac_part.push((b'0' + digit_at(point + i)) as char);
    }
    while frac_part.len() > min_fraction && frac_part.ends_with('0') {
        frac_part.pop();
    }

    let mut grouped = String::new();
    let int_len = int_part.len();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_len - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    // ICU keeps the sign of negative numbers that round to zero, and of -0.
    let mut result = String::new();
    if number.is_sign_negative() {
        result.push('-');
    }
    if let Some(symbol) = symbol {
        result.push_str(&symbol);
    }
    result.push_str(&grouped);
    if !frac_part.is_empty() {
        result.push('.');
        result.push_str(&frac_part);
    }
    result
}

/// Digits and decimal point position of a `{:e}` formatting.
fn sci_digits(sci: &str) -> (Vec<u8>, i64) {
    let (mantissa, exponent) = sci.split_once('e').unwrap();
    let exponent: i64 = exponent.parse().unwrap();
    let digits = mantissa.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    (digits, exponent + 1)
}

/// The shortest round-trip decimal digits of `x`, as double-conversion's
/// SHORTEST mode (which ICU's `DecimalQuantity` uses) produces them: when two
/// shortest candidates are exactly equally close, the even one (Rust's
/// formatter may pick the other).
fn shortest_digits(x: f64) -> (Vec<u8>, i64) {
    let (digits, point) = sci_digits(&format!("{x:e}"));
    let (mut exact, exact_point) = sci_digits(&format!("{x:.1100e}"));
    while exact.len() > 1 && exact.last() == Some(&0) {
        exact.pop();
    }
    let l = digits.len();
    if exact_point == point && exact.len() == l + 1 && exact[l] == 5 && l <= 17 {
        let to_int = |d: &[u8]| d.iter().fold(0u64, |a, d| a * 10 + u64::from(*d));
        let low = to_int(&exact[..l]);
        let shortest = to_int(&digits);
        if shortest == low || shortest == low + 1 {
            let even = if low % 2 == 0 { low } else { low + 1 };
            let mut out: Vec<u8> = even.to_string().bytes().map(|b| b - b'0').collect();
            let mut p = point;
            if out.len() > l {
                p += 1;
            }
            while out.len() < l {
                out.insert(0, 0);
            }
            while out.len() > 1 && out.last() == Some(&0) {
                out.pop();
            }
            return (out, p);
        }
    }
    (digits, point)
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

        let mut evaluated_min_fraction_digits: u8 = 0;
        if let Some(min) = &self.min_fraction_digits {
            evaluated_min_fraction_digits = static_cast_u8(min.evaluate(params)?.as_number().expect("number"));
        }

        let mut evaluated_max_fraction_digits: u8 = 3;
        if let Some(max) = &self.max_fraction_digits {
            evaluated_max_fraction_digits = static_cast_u8(max.evaluate(params)?.as_number().expect("number"));
        }

        Ok(Value::String(format_number(
            evaluated_number,
            &evaluated_locale,
            &evaluated_currency,
            evaluated_min_fraction_digits,
            evaluated_max_fraction_digits,
        )))
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

    #[test]
    fn icu_like() {
        assert_eq!(format_number(123_456.789, "", "", 0, 3), "123,456.789");
        assert_eq!(format_number(123_456.789, "en-US", "JPY", 0, 3), "¥123,457");
        assert_eq!(format_number(123_456.789, "en-US", "EUR", 0, 3), "€123,456.79");
        assert_eq!(format_number(987_654_321.234_567, "en-US", "", 15, 20), "987,654,321.234567000000000");
        assert_eq!(format_number(987_654_321.234_567, "en-US", "", 2, 4), "987,654,321.2346");
        assert_eq!(format_number(0.5, "", "", 0, 0), "0");
        assert_eq!(format_number(1.5, "", "", 0, 0), "2");
        assert_eq!(format_number(0.0005, "", "", 0, 3), "0");
    }
}
