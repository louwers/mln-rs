//! The locale services on Apple platforms: Foundation's, with the system's
//! locale data. The default locale is the user's current locale.
//!
//! Foundation's localized comparison (`compare:options:range:locale:`) has
//! neither collation types nor a case-first setting, so a collator honors
//! only the `kn` extension, as `NSNumericSearch`: `co` and `kf` have no
//! effect, and its resolved locale leaves them out. Its case- and
//! diacritic-insensitive options select the strength.

#![expect(unsafe_code, reason = "Foundation through objc2 is the FFI here")]

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFNumberFormatter, CFString};
use objc2_foundation::{
    NSLocale, NSNumber, NSNumberFormatter, NSNumberFormatterRoundingMode, NSNumberFormatterStyle, NSRange, NSString,
    NSStringCompareOptions,
};

use super::keywords::CollationKeywords;
use super::{FractionDigits, Sensitivity};

/// The language of an ICU locale identifier or a language tag.
fn language_of(identifier: &str) -> &str {
    identifier.split(['_', '-', '@']).next().unwrap_or_default()
}

/// The languages Foundation has locale data for.
fn available_languages() -> &'static HashSet<String> {
    static LANGUAGES: OnceLock<HashSet<String>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        NSLocale::availableLocaleIdentifiers()
            .to_vec()
            .into_iter()
            .map(|identifier| language_of(&identifier.to_string()).to_owned())
            .collect()
    })
}

/// The locale a BCP 47 tag names, with its Unicode extensions as keywords,
/// or the current locale when the tag is empty or its language has no data.
fn resolve_locale(tag: &str) -> Retained<NSLocale> {
    if !tag.is_empty() {
        let identifier = NSLocale::canonicalLocaleIdentifierFromString(&NSString::from_str(tag));
        let locale = NSLocale::localeWithLocaleIdentifier(&identifier);
        if available_languages().contains(&locale.languageCode().to_string()) {
            return locale;
        }
    }
    NSLocale::currentLocale()
}

/// A locale's language, script and region as a language tag, without its
/// keywords, which its identifier may carry after `@` or, since macOS 26, as
/// a `-u-` extension.
fn base_tag(locale: &NSLocale) -> String {
    let mut tag = locale.languageCode().to_string();
    if let Some(script) = locale.scriptCode() {
        tag.push('-');
        tag.push_str(&script.to_string());
    }
    #[expect(deprecated, reason = "regionCode needs macOS 13 and iOS 16")]
    if let Some(region) = locale.countryCode() {
        tag.push('-');
        tag.push_str(&region.to_string());
    }
    tag
}

/// A Foundation collator: a locale without keywords and the comparison's
/// options.
pub(crate) struct Collator {
    locale: Retained<NSLocale>,
    options: NSStringCompareOptions,
    resolved_locale: String,
}

impl Collator {
    #[expect(clippy::unnecessary_wraps, reason = "the other platforms' collators can fail")]
    pub(crate) fn new(tag: &str, sensitivity: Sensitivity) -> Result<Self, String> {
        let language = base_tag(&resolve_locale(tag));
        let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(&language));

        let numeric = CollationKeywords::parse(tag).numeric;
        let mut options = NSStringCompareOptions::empty();
        if !sensitivity.case {
            options |= NSStringCompareOptions::CaseInsensitiveSearch;
        }
        if !sensitivity.diacritics {
            options |= NSStringCompareOptions::DiacriticInsensitiveSearch;
        }
        if numeric == Some(true) {
            options |= NSStringCompareOptions::NumericSearch;
        }
        let honored = CollationKeywords { numeric, ..CollationKeywords::default() };
        Ok(Collator { locale, options, resolved_locale: honored.append_to(&language) })
    }

    pub(crate) fn compare(&self, lhs: &str, rhs: &str) -> Ordering {
        let lhs = NSString::from_str(lhs);
        let rhs = NSString::from_str(rhs);
        let locale: &AnyObject = &self.locale;
        // SAFETY: the locale is an NSLocale, and the range is the whole
        // receiver.
        unsafe { lhs.compare_options_range_locale(&rhs, self.options, NSRange::new(0, lhs.length()), Some(locale)) }
            .into()
    }

    pub(crate) fn resolved_locale(&self) -> &str {
        &self.resolved_locale
    }
}

/// The ISO 4217 fraction digits of a currency, or 2 for one Foundation
/// doesn't know.
fn currency_digits(code: &str) -> u8 {
    let mut digits = 2;
    let mut rounding_increment = 0.0;
    // SAFETY: the pointers are to live locals.
    let known = unsafe {
        CFNumberFormatter::decimal_info_for_currency_code(
            Some(&CFString::from_str(code)),
            &raw mut digits,
            &raw mut rounding_increment,
        )
    };
    if known { u8::try_from(digits).unwrap_or(2) } else { 2 }
}

/// Formats with an `NSNumberFormatter` in the decimal or currency style.
pub(crate) fn format_number(
    number: f64,
    tag: &str,
    currency: Option<&str>,
    digits: FractionDigits,
) -> Result<String, String> {
    let formatter = NSNumberFormatter::new();
    formatter.setLocale(Some(&resolve_locale(tag)));
    let currency_digits = if let Some(code) = currency {
        formatter.setNumberStyle(NSNumberFormatterStyle::CurrencyStyle);
        formatter.setCurrencyCode(Some(&NSString::from_str(code)));
        Some(currency_digits(code))
    } else {
        formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        None
    };
    let (min, max) = digits.resolve(currency_digits);
    // The maximum first: setting a limit moves the other past it.
    formatter.setMaximumFractionDigits(max.into());
    formatter.setMinimumFractionDigits(min.into());
    formatter.setRoundingMode(NSNumberFormatterRoundingMode::RoundHalfEven);
    // Foundation's own is "+∞".
    formatter.setPositiveInfinitySymbol(&NSString::from_str("∞"));
    formatter
        .stringFromNumber(&NSNumber::numberWithDouble(number))
        .map(|formatted| formatted.to_string())
        .ok_or_else(|| format!("NSNumberFormatter can't format {number}"))
}
