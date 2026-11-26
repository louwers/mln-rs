//! The locale services on Apple platforms: Foundation's and the system ICU's,
//! with the system's locale data. The default locale is the user's current
//! locale.
//!
//! Foundation's localized comparison (`compare:options:range:locale:`) has
//! neither collation types nor a case-first setting, so a collator honors
//! only the `kn` extension, as `NSNumericSearch`: `co` and `kf` have no
//! effect, and its resolved locale leaves them out. Its case- and
//! diacritic-insensitive options select the strength.
//!
//! A number formats with the system's ICU, `libicucore`, through its number
//! skeletons, as Safari's `Intl.NumberFormat` does. That needs macOS 10.14
//! or iOS 12, whose ICU has no `petabyte` and no `percent` denominator.
//!
//! Each use of Foundation drains an autorelease pool of its own: expressions
//! are evaluated on threads that have none, and Foundation hands its locales
//! and strings back through one.

#![expect(unsafe_code, reason = "Foundation through objc2 and ICU are the FFI here")]

use std::cmp::Ordering;
use std::ffi::CString;
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSLocale, NSRange, NSString, NSStringCompareOptions};

use super::keywords::CollationKeywords;
use super::{FractionDigits, Sensitivity, Style, Unit, default_locale_override};
use crate::hash::HashSet;

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
/// unless the tag is empty or its language has no data.
fn named_locale(tag: &str) -> Option<Retained<NSLocale>> {
    if tag.is_empty() {
        return None;
    }
    let identifier = NSLocale::canonicalLocaleIdentifierFromString(&NSString::from_str(tag));
    let locale = NSLocale::localeWithLocaleIdentifier(&identifier);
    available_languages().contains(&locale.languageCode().to_string()).then_some(locale)
}

/// The locale a BCP 47 tag names, or the default locale when it names none.
fn resolve_locale(tag: &str) -> Retained<NSLocale> {
    named_locale(tag)
        .or_else(|| default_locale_override().and_then(named_locale))
        .unwrap_or_else(NSLocale::currentLocale)
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
        let (language, locale) = autoreleasepool(|_| {
            let language = base_tag(&resolve_locale(tag));
            let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(&language));
            (language, locale)
        });

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

/// The bindings to the system's ICU, `libicucore`, which ships with every
/// Apple OS but whose headers the SDKs leave out. Apple builds it without
/// ICU's versioned symbol names.
mod icu {
    use std::ffi::c_char;

    /// An ICU status: positive for an error, negative for a warning.
    pub(super) type UErrorCode = i32;
    pub(super) const U_ZERO_ERROR: UErrorCode = 0;
    pub(super) const U_BUFFER_OVERFLOW_ERROR: UErrorCode = 15;

    #[repr(C)]
    pub(super) struct UNumberFormatter {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub(super) struct UFormattedNumber {
        _private: [u8; 0],
    }

    #[link(name = "icucore", kind = "dylib")]
    unsafe extern "C" {
        pub(super) fn unumf_openForSkeletonAndLocale(
            skeleton: *const u16,
            skeleton_len: i32,
            locale: *const c_char,
            status: *mut UErrorCode,
        ) -> *mut UNumberFormatter;
        pub(super) fn unumf_close(formatter: *mut UNumberFormatter);
        pub(super) fn unumf_openResult(status: *mut UErrorCode) -> *mut UFormattedNumber;
        pub(super) fn unumf_closeResult(result: *mut UFormattedNumber);
        pub(super) fn unumf_formatDouble(
            formatter: *const UNumberFormatter,
            value: f64,
            result: *mut UFormattedNumber,
            status: *mut UErrorCode,
        );
        pub(super) fn unumf_resultToString(
            result: *const UFormattedNumber,
            buffer: *mut u16,
            capacity: i32,
            status: *mut UErrorCode,
        ) -> i32;
        pub(super) fn ucurr_getDefaultFractionDigits(currency: *const u16, status: *mut UErrorCode) -> i32;
    }
}

/// The ISO 4217 fraction digits of a currency: 2 for one ICU doesn't know.
fn currency_digits(code: &str) -> u8 {
    let code: Vec<u16> = code.encode_utf16().chain([0]).collect();
    let mut status = icu::U_ZERO_ERROR;
    // SAFETY: the code is a NUL-terminated UTF-16 string.
    let digits = unsafe { icu::ucurr_getDefaultFractionDigits(code.as_ptr(), &raw mut status) };
    if status > 0 { 2 } else { u8::try_from(digits).unwrap_or(2) }
}

/// The type ICU's skeletons qualify a simple unit with.
fn unit_type(unit: &str) -> Result<&'static str, String> {
    Ok(match unit {
        "acre" | "hectare" => "area",
        "bit" | "byte" | "gigabit" | "gigabyte" | "kilobit" | "kilobyte" | "megabit" | "megabyte" | "petabyte"
        | "terabit" | "terabyte" => "digital",
        "celsius" | "fahrenheit" => "temperature",
        "centimeter" | "foot" | "inch" | "kilometer" | "meter" | "mile" | "mile-scandinavian" | "millimeter"
        | "yard" => "length",
        "day" | "hour" | "microsecond" | "millisecond" | "minute" | "month" | "nanosecond" | "second" | "week"
        | "year" => "duration",
        "degree" => "angle",
        "fluid-ounce" | "gallon" | "liter" | "milliliter" => "volume",
        "gram" | "kilogram" | "ounce" | "pound" | "stone" => "mass",
        "percent" => "concentr",
        _ => return Err(format!("{unit} has no ICU unit type")),
    })
}

/// The ICU number skeleton of a style and fraction limits.
fn skeleton(style: Style<'_>, digits: FractionDigits) -> Result<String, String> {
    let (stem, currency_digits) = match style {
        Style::Decimal => (String::new(), None),
        Style::Currency(code) => (format!("currency/{code} "), Some(currency_digits(code))),
        Style::Unit(Unit { unit, per }) => {
            let per = match per {
                Some(per) => format!("per-measure-unit/{}-{per} ", unit_type(per)?),
                None => String::new(),
            };
            // ICU 62 has `percent` only as this stem, not as a measure unit.
            let unit = if unit == "percent" {
                "percent".to_owned()
            } else {
                format!("measure-unit/{}-{unit}", unit_type(unit)?)
            };
            (format!("{unit} {per}"), None)
        }
    };
    let (min, max) = digits.resolve(currency_digits);
    let precision = if max == 0 {
        "precision-integer".to_owned()
    } else {
        format!(".{}{}", "0".repeat(min.into()), "#".repeat((max - min).into()))
    };
    Ok(stem + &precision)
}

/// An open ICU number formatter.
struct NumberFormatter(NonNull<icu::UNumberFormatter>);

impl Drop for NumberFormatter {
    fn drop(&mut self) {
        // SAFETY: the formatter is open, and closes only here.
        unsafe { icu::unumf_close(self.0.as_ptr()) }
    }
}

/// An open ICU formatted number.
struct FormattedNumber(NonNull<icu::UFormattedNumber>);

impl Drop for FormattedNumber {
    fn drop(&mut self) {
        // SAFETY: the result is open, and closes only here.
        unsafe { icu::unumf_closeResult(self.0.as_ptr()) }
    }
}

/// Fails on an ICU error, with what failed.
fn check(status: icu::UErrorCode, what: &str) -> Result<(), String> {
    if status > 0 { Err(format!("{what} failed with ICU error {status}")) } else { Ok(()) }
}

/// Formats with the system ICU's number formatter, as Safari's
/// `Intl.NumberFormat` does.
pub(crate) fn format_number(
    number: f64,
    tag: &str,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<String, String> {
    let locale = autoreleasepool(|_| resolve_locale(tag).localeIdentifier().to_string());
    let locale = CString::new(locale).map_err(|error| error.to_string())?;
    let skeleton: Vec<u16> = skeleton(style, digits)?.encode_utf16().collect();
    let skeleton_len = i32::try_from(skeleton.len()).map_err(|error| error.to_string())?;

    let mut status = icu::U_ZERO_ERROR;
    // SAFETY: the skeleton is `skeleton_len` UTF-16 units, and the locale a
    // NUL-terminated string.
    let formatter = unsafe {
        icu::unumf_openForSkeletonAndLocale(skeleton.as_ptr(), skeleton_len, locale.as_ptr(), &raw mut status)
    };
    let formatter = NonNull::new(formatter).map(NumberFormatter);
    check(status, "Opening a number formatter")?;
    let formatter = formatter.ok_or("ICU opened no number formatter")?;

    // SAFETY: the status is a live local.
    let result = NonNull::new(unsafe { icu::unumf_openResult(&raw mut status) }).map(FormattedNumber);
    check(status, "Opening a formatted number")?;
    let result = result.ok_or("ICU opened no formatted number")?;

    // SAFETY: the formatter and the result are open.
    unsafe { icu::unumf_formatDouble(formatter.0.as_ptr(), number, result.0.as_ptr(), &raw mut status) };
    check(status, "Formatting a number")?;

    let mut buffer = vec![0; 64];
    loop {
        let capacity = i32::try_from(buffer.len()).map_err(|error| error.to_string())?;
        // SAFETY: the result is open, and the buffer holds `capacity` units.
        let len =
            unsafe { icu::unumf_resultToString(result.0.as_ptr(), buffer.as_mut_ptr(), capacity, &raw mut status) };
        let len = usize::try_from(len).unwrap_or_default();
        if status == icu::U_BUFFER_OVERFLOW_ERROR {
            status = icu::U_ZERO_ERROR;
            buffer.resize(len, 0);
            continue;
        }
        check(status, "Reading a formatted number")?;
        return Ok(String::from_utf16_lossy(&buffer[..len]));
    }
}
