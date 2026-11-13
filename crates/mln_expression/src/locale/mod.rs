//! The locale services of the locale-sensitive expressions (`collator`,
//! `resolved-locale` and `number-format`). Like the HTTP transport, they are
//! the platform's where it has them, else a crate:
//!
//! - Apple platforms: Foundation's localized string comparison,
//!   `NSNumberFormatter` and `NSLocale`.
//! - wasm (the browser build): the browser's `Intl.Collator` and
//!   `Intl.NumberFormat`.
//! - Everywhere else: ICU4X, with the CLDR data it compiles into the library.
//!
//! Each backend provides a `Collator` and `format_number` with the same
//! contract, documented on the backend where it falls short:
//!
//! - An empty, malformed or unsupported language tag takes the platform's
//!   default locale.
//! - A collator compares at the strength the sensitivity selects: base
//!   letters only, then diacritics, then case.
//! - The `co` (collation type), `kf` (case first) and `kn` (numeric) Unicode
//!   extensions of the tag select the collator's order where the platform
//!   supports them. The collator's resolved locale is the language it
//!   compares in, with the extensions it honors.
//! - A number formats with the locale's digits, separators and grouping,
//!   rounded half to even to the fraction limits. With a currency, it takes
//!   the locale's currency pattern and symbol, and the currency's digits
//!   unless the limits override them.

#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(not(any(target_family = "wasm", target_vendor = "apple")))]
mod icu4x;
#[cfg(any(target_family = "wasm", target_vendor = "apple", test))]
mod keywords;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(target_vendor = "apple")]
pub(crate) use apple::{Collator, format_number};
#[cfg(not(any(target_family = "wasm", target_vendor = "apple")))]
pub(crate) use icu4x::{Collator, format_number};
#[cfg(target_family = "wasm")]
pub(crate) use web::{Collator, format_number};

/// What a collator tells apart beyond base letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Sensitivity {
    pub(crate) case: bool,
    pub(crate) diacritics: bool,
}

/// The fraction digit limits of a number format, as the expression gives
/// them, with an explicit maximum winning over a larger minimum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct FractionDigits {
    pub(crate) min: Option<u8>,
    pub(crate) max: Option<u8>,
}

impl FractionDigits {
    pub(crate) fn new(min: Option<u8>, max: Option<u8>) -> Self {
        let min = match (min, max) {
            (Some(min), Some(max)) => Some(min.min(max)),
            _ => min,
        };
        FractionDigits { min, max }
    }

    /// The minimum and maximum, with the defaults for the omitted ones: 0
    /// and 3, or the currency's digits for currency. An omitted maximum is at
    /// least the minimum, and an omitted minimum at most the maximum.
    #[cfg_attr(
        all(target_family = "wasm", not(test)),
        expect(dead_code, reason = "Intl.NumberFormat applies the defaults itself")
    )]
    pub(crate) fn resolve(self, currency_digits: Option<u8>) -> (u8, u8) {
        let (default_min, default_max) = currency_digits.map_or((0, 3), |digits| (digits, digits));
        let max = self.max.unwrap_or_else(|| default_max.max(self.min.unwrap_or(0)));
        let min = self.min.unwrap_or(default_min).min(max);
        (min, max)
    }
}

/// A `number-format` currency as an ISO 4217 code: three ASCII letters, in
/// upper case. Empty means none.
///
/// # Errors
///
/// When the currency isn't three ASCII letters.
pub(crate) fn currency_code(currency: &str) -> Result<Option<String>, String> {
    if currency.is_empty() {
        return Ok(None);
    }
    if currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        Ok(Some(currency.to_ascii_uppercase()))
    } else {
        Err(format!("{currency} is not an ISO 4217 currency code"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_digits_take_the_defaults_and_an_explicit_maximum_wins() {
        assert_eq!(FractionDigits::new(None, None).resolve(None), (0, 3));
        assert_eq!(FractionDigits::new(Some(5), None).resolve(None), (5, 5));
        assert_eq!(FractionDigits::new(None, Some(1)).resolve(Some(2)), (1, 1));
        assert_eq!(FractionDigits::new(Some(4), Some(2)).resolve(None), (2, 2));
        assert_eq!(FractionDigits::new(None, None).resolve(Some(0)), (0, 0));
        assert_eq!(FractionDigits::new(Some(3), None).resolve(Some(2)), (3, 3));
    }

    #[test]
    fn currency_codes_are_three_letters() {
        assert_eq!(currency_code(""), Ok(None));
        assert_eq!(currency_code("usd"), Ok(Some("USD".to_owned())));
        assert!(currency_code("US$").is_err());
        assert!(currency_code("EURO").is_err());
    }
}
