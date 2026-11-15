//! The locale services on Apple platforms: Foundation's, with the system's
//! locale data. The default locale is the user's current locale.
//!
//! Foundation's localized comparison (`compare:options:range:locale:`) has
//! neither collation types nor a case-first setting, so a collator honors
//! only the `kn` extension, as `NSNumericSearch`: `co` and `kf` have no
//! effect, and its resolved locale leaves them out. Its case- and
//! diacritic-insensitive options select the strength.
//!
//! A unit formats with `NSMeasurementFormatter` in its medium style, which is
//! CLDR's short one, and `percent` with `NSNumberFormatter`'s percent style.
//! Foundation has no unit for `day`, `week`, `month` or `year`, and of the
//! compound units only for `kilometer-per-hour`, `meter-per-second`,
//! `mile-per-hour` and `mile-per-gallon`, so a `number-format` with any other
//! fails. Neither do macOS before 10.15 and iOS before 13 have units of
//! digital information or durations shorter than a second.
//!
//! Each entry point drains an autorelease pool of its own: expressions are
//! evaluated on threads that have none, and Foundation hands its locales and
//! strings back through one.

#![expect(unsafe_code, reason = "Foundation through objc2 is the FFI here")]

use std::cmp::Ordering;
use std::sync::OnceLock;

use mln_util::hash::HashSet;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2::{AnyThread, ClassType, available};
use objc2_core_foundation::{CFNumberFormatter, CFString};
use objc2_foundation::{
    NSDimension, NSFormattingUnitStyle, NSLocale, NSMeasurement, NSMeasurementFormatter,
    NSMeasurementFormatterUnitOptions, NSNumber, NSNumberFormatter, NSNumberFormatterRoundingMode,
    NSNumberFormatterStyle, NSRange, NSString, NSStringCompareOptions, NSUnit, NSUnitAngle, NSUnitArea, NSUnitDuration,
    NSUnitFuelEfficiency, NSUnitInformationStorage, NSUnitLength, NSUnitMass, NSUnitSpeed, NSUnitTemperature,
    NSUnitVolume,
};

use super::keywords::CollationKeywords;
use super::{FractionDigits, Sensitivity, Style, Unit};

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

fn dimension<T: ClassType<Super = NSDimension> + 'static>(unit: Retained<T>) -> Retained<NSDimension> {
    Retained::into_super(unit)
}

/// Foundation's unit for a unit, other than `percent`, if it has one.
fn foundation_unit(unit: Unit) -> Option<Retained<NSDimension>> {
    // Units of digital information and those durations are in macOS 10.15
    // and iOS 13.
    let recent = available!(macos = 10.15, ios = 13.0, tvos = 13.0, watchos = 6.0, visionos = 1.0, ..);
    let simple = match unit.unit {
        "acre" => dimension(NSUnitArea::acres()),
        "celsius" => dimension(NSUnitTemperature::celsius()),
        "centimeter" => dimension(NSUnitLength::centimeters()),
        "degree" => dimension(NSUnitAngle::degrees()),
        "fahrenheit" => dimension(NSUnitTemperature::fahrenheit()),
        "fluid-ounce" => dimension(NSUnitVolume::fluidOunces()),
        "foot" => dimension(NSUnitLength::feet()),
        "gallon" => dimension(NSUnitVolume::gallons()),
        "gram" => dimension(NSUnitMass::grams()),
        "hectare" => dimension(NSUnitArea::hectares()),
        "hour" => dimension(NSUnitDuration::hours()),
        "inch" => dimension(NSUnitLength::inches()),
        "kilogram" => dimension(NSUnitMass::kilograms()),
        "kilometer" => dimension(NSUnitLength::kilometers()),
        "liter" => dimension(NSUnitVolume::liters()),
        "meter" => dimension(NSUnitLength::meters()),
        "mile" => dimension(NSUnitLength::miles()),
        "mile-scandinavian" => dimension(NSUnitLength::scandinavianMiles()),
        "milliliter" => dimension(NSUnitVolume::milliliters()),
        "millimeter" => dimension(NSUnitLength::millimeters()),
        "minute" => dimension(NSUnitDuration::minutes()),
        "ounce" => dimension(NSUnitMass::ounces()),
        "pound" => dimension(NSUnitMass::poundsMass()),
        "second" => dimension(NSUnitDuration::seconds()),
        "stone" => dimension(NSUnitMass::stones()),
        "yard" => dimension(NSUnitLength::yards()),
        "microsecond" if recent => dimension(NSUnitDuration::microseconds()),
        "millisecond" if recent => dimension(NSUnitDuration::milliseconds()),
        "nanosecond" if recent => dimension(NSUnitDuration::nanoseconds()),
        "bit" if recent => dimension(NSUnitInformationStorage::bits()),
        "byte" if recent => dimension(NSUnitInformationStorage::bytes()),
        "gigabit" if recent => dimension(NSUnitInformationStorage::gigabits()),
        "gigabyte" if recent => dimension(NSUnitInformationStorage::gigabytes()),
        "kilobit" if recent => dimension(NSUnitInformationStorage::kilobits()),
        "kilobyte" if recent => dimension(NSUnitInformationStorage::kilobytes()),
        "megabit" if recent => dimension(NSUnitInformationStorage::megabits()),
        "megabyte" if recent => dimension(NSUnitInformationStorage::megabytes()),
        "petabyte" if recent => dimension(NSUnitInformationStorage::petabytes()),
        "terabit" if recent => dimension(NSUnitInformationStorage::terabits()),
        "terabyte" if recent => dimension(NSUnitInformationStorage::terabytes()),
        _ => return None,
    };
    match unit.per {
        None => Some(simple),
        Some(per) => Some(match (unit.unit, per) {
            ("kilometer", "hour") => dimension(NSUnitSpeed::kilometersPerHour()),
            ("meter", "second") => dimension(NSUnitSpeed::metersPerSecond()),
            ("mile", "hour") => dimension(NSUnitSpeed::milesPerHour()),
            ("mile", "gallon") => dimension(NSUnitFuelEfficiency::milesPerGallon()),
            _ => return None,
        }),
    }
}

/// Formats with an `NSNumberFormatter` in the decimal, currency or percent
/// style, or with an `NSMeasurementFormatter` over one in the decimal style.
pub(crate) fn format_number(
    number: f64,
    tag: &str,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<String, String> {
    autoreleasepool(|_| {
        let locale = resolve_locale(tag);
        let formatter = NSNumberFormatter::new();
        formatter.setLocale(Some(&locale));
        let mut measure = None;
        let currency_digits = match style {
            Style::Decimal => {
                formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
                None
            }
            Style::Currency(code) => {
                formatter.setNumberStyle(NSNumberFormatterStyle::CurrencyStyle);
                formatter.setCurrencyCode(Some(&NSString::from_str(code)));
                Some(currency_digits(code))
            }
            // As in Intl, a number in percent is already a percentage.
            Style::Unit(Unit { unit: "percent", per: None }) => {
                formatter.setNumberStyle(NSNumberFormatterStyle::PercentStyle);
                formatter.setMultiplier(Some(&NSNumber::numberWithInt(1)));
                None
            }
            Style::Unit(unit) => {
                let foundation_unit =
                    foundation_unit(unit).ok_or_else(|| format!("{unit} can't be formatted on this platform"))?;
                measure = Some(foundation_unit);
                formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
                None
            }
        };
        let (min, max) = digits.resolve(currency_digits);
        // The maximum first: setting a limit moves the other past it.
        formatter.setMaximumFractionDigits(max.into());
        formatter.setMinimumFractionDigits(min.into());
        formatter.setRoundingMode(NSNumberFormatterRoundingMode::RoundHalfEven);
        // Foundation's own is "+∞".
        formatter.setPositiveInfinitySymbol(&NSString::from_str("∞"));
        let Some(unit) = measure else {
            return formatter
                .stringFromNumber(&NSNumber::numberWithDouble(number))
                .map(|formatted| formatted.to_string())
                .ok_or_else(|| format!("NSNumberFormatter can't format {number}"));
        };
        let measurement_formatter = NSMeasurementFormatter::new();
        measurement_formatter.setLocale(Some(&locale));
        measurement_formatter.setUnitOptions(NSMeasurementFormatterUnitOptions::ProvidedUnit);
        measurement_formatter.setUnitStyle(NSFormattingUnitStyle::Medium);
        measurement_formatter.setNumberFormatter(Some(&formatter));
        let measurement = NSMeasurement::<NSUnit>::initWithDoubleValue_unit(NSMeasurement::alloc(), number, &unit);
        // SAFETY: NSMeasurement's type parameter only types its unit, and the
        // formatter takes a measurement of a dimension.
        let formatted = unsafe {
            let measurement: Retained<NSMeasurement> = Retained::cast_unchecked(measurement);
            measurement_formatter.stringFromMeasurement(&measurement)
        };
        Ok(formatted.to_string())
    })
}
