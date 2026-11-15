//! The locale services on platforms without their own (Linux, Windows,
//! Android): ICU4X, with the CLDR data it compiles into the library. The
//! default locale comes from the POSIX environment.
//!
//! A unit takes its short CLDR name from `icu_experimental`, which has names
//! only for units of length, area, volume, mass and duration, and `percent`
//! takes CLDR's percent pattern. A `number-format` with another unit fails:
//! `celsius`, `fahrenheit`, `degree`, the units of digital information, and
//! every compound unit.

use std::cmp::Ordering;
use std::sync::OnceLock;

use fixed_decimal::{Decimal, FloatPrecision, Sign};
use icu_collator::options::{CaseLevel, CollatorOptions, Strength};
use icu_collator::provider::CollationMetadataV1;
use icu_collator::{CollatorBorrowed, CollatorPreferences};
use icu_decimal::options::DecimalFormatterOptions;
use icu_decimal::{AbstractFormatter, DecimalFormatter, DecimalFormatterPreferences};
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::provider::currency::essentials::CurrencyEssentialsV1;
use icu_experimental::dimension::provider::currency::fractions::CurrencyFractionsV1;
use icu_experimental::dimension::provider::currency::symbols::CurrencySymbolsV1;
use icu_experimental::dimension::provider::percent::DecimalPercentV1;
use icu_experimental::dimension::provider::units::categorized_display_names::{
    UnitsNamesAreaCoreV1, UnitsNamesAreaExtendedV1, UnitsNamesAreaOutlierV1, UnitsNamesDurationCoreV1,
    UnitsNamesDurationExtendedV1, UnitsNamesDurationOutlierV1, UnitsNamesLengthCoreV1, UnitsNamesLengthExtendedV1,
    UnitsNamesLengthOutlierV1, UnitsNamesMassCoreV1, UnitsNamesMassExtendedV1, UnitsNamesMassOutlierV1,
    UnitsNamesVolumeCoreV1, UnitsNamesVolumeExtendedV1, UnitsNamesVolumeOutlierV1,
};
use icu_experimental::dimension::provider::units::display_names::UnitsDisplayNames;
use icu_locale::extensions::unicode::key;
use icu_locale::preferences::LocalePreferences;
use icu_locale::subtags::script;
use icu_locale::{Locale, LocaleCanonicalizer, LocaleExpander, locale};
use icu_plurals::{PluralRules, PluralRulesPreferences};
use icu_provider::marker::ErasedMarker;
use icu_provider::prelude::*;
use writeable::Writeable;

use super::{FractionDigits, Sensitivity, Style, Unit};

/// The POSIX locale category whose environment variable names the default
/// locale of an expression.
#[derive(Clone, Copy, Debug)]
enum Category {
    /// `LC_COLLATE`, for collators.
    Collate,
    /// `LC_NUMERIC`, for number formats.
    Numeric,
}

impl Category {
    fn variable(self) -> &'static str {
        match self {
            Category::Collate => "LC_COLLATE",
            Category::Numeric => "LC_NUMERIC",
        }
    }

    /// The default locale, which the environment fixes for the process.
    fn default_locale(self) -> &'static Locale {
        static COLLATE: OnceLock<Locale> = OnceLock::new();
        static NUMERIC: OnceLock<Locale> = OnceLock::new();
        let cell = match self {
            Category::Collate => &COLLATE,
            Category::Numeric => &NUMERIC,
        };
        cell.get_or_init(|| environment_locale(self.variable(), |name| std::env::var(name).ok()))
    }
}

/// The locale the environment names for a POSIX locale category, as the first
/// nonempty of `LC_ALL`, the category and `LANG`, such as `sv_SE.UTF-8` or
/// `sr_RS.UTF-8@latin`. `C`, `POSIX`, and an unset or unsupported locale mean
/// `en-US`, as does the browser, which has no environment.
fn environment_locale(category: &str, variable: impl Fn(&str) -> Option<String>) -> Locale {
    let name =
        ["LC_ALL", category, "LANG"].into_iter().find_map(|name| variable(name).filter(|value| !value.is_empty()));
    let name = name.unwrap_or_default();
    let (name, modifier) = name.split_once('@').unwrap_or((&name, ""));
    let tag = name.split('.').next().unwrap_or_default();
    let Some(mut locale) = supported_locale(&tag.replace('_', "-")) else {
        return locale!("en-US");
    };
    // glibc names a language's non-default script with a modifier.
    locale.id.script = match modifier {
        "cyrillic" => Some(script!("Cyrl")),
        "devanagari" => Some(script!("Deva")),
        "latin" => Some(script!("Latn")),
        _ => locale.id.script,
    };
    locale
}

/// Parses and canonicalizes a language tag whose language CLDR knows. Data
/// lookups would fall back to the root locale's data for an unknown language,
/// data that supported languages such as English share.
fn supported_locale(tag: &str) -> Option<Locale> {
    let mut locale = Locale::try_from_str(tag).ok()?;
    LocaleCanonicalizer::new_common().canonicalize(&mut locale);
    let mut language = locale.id.clone();
    language.script = None;
    language.region = None;
    LocaleExpander::new_common().maximize(&mut language);
    language.script.is_some().then_some(locale)
}

/// Resolves the language tag an expression requests. An empty or unsupported
/// tag takes the default locale's language, keeping the requested Unicode
/// extensions.
fn resolve_locale(requested: &str, category: Category) -> Locale {
    supported_locale(requested).unwrap_or_else(|| {
        let mut locale = Locale::try_from_str(requested).unwrap_or(Locale::UNKNOWN);
        locale.id = category.default_locale().id.clone();
        locale
    })
}

/// Whether the compiled data holds the collation type that `prefs` requests,
/// such as German's `phonebk`.
fn has_collation_type(prefs: &CollatorPreferences) -> bool {
    let Some(attributes) =
        prefs.collation_type.as_ref().and_then(|kind| DataMarkerAttributes::try_from_str(kind.as_str()).ok())
    else {
        return false;
    };
    let locale = CollationMetadataV1::make_locale(prefs.locale_preferences);
    let request = silent_request(DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale));
    DataProvider::<CollationMetadataV1>::load(&icu_collator::provider::Baked, request).is_ok()
}

/// A data request whose miss the caller handles, so ICU4X doesn't log it.
fn silent_request(id: DataIdentifierBorrowed<'_>) -> DataRequest<'_> {
    let mut metadata = DataRequestMetadata::default();
    metadata.silent = true;
    DataRequest { id, metadata }
}

/// An ICU4X collator. Case without diacritics is primary strength with the
/// case level on. The `co` extension counts as honored only when the data
/// has that collation type for the language.
pub(crate) struct Collator {
    collator: CollatorBorrowed<'static>,
    resolved_locale: String,
}

impl Collator {
    pub(crate) fn new(locale: &str, sensitivity: Sensitivity) -> Result<Self, String> {
        let locale = resolve_locale(locale, Category::Collate);
        let prefs = CollatorPreferences::from(&locale);
        let mut resolved = Locale::from(locale.id.clone());
        for (keyword, honored) in [
            (key!("co"), has_collation_type(&prefs)),
            (key!("kf"), prefs.case_first.is_some()),
            (key!("kn"), prefs.numeric_ordering.is_some()),
        ] {
            if honored && let Some(value) = locale.extensions.unicode.keywords.get(&keyword) {
                resolved.extensions.unicode.keywords.set(keyword, value.clone());
            }
        }

        let mut options = CollatorOptions::default();
        options.strength = Some(match (sensitivity.diacritics, sensitivity.case) {
            (false, _) => Strength::Primary,
            (true, false) => Strength::Secondary,
            (true, true) => Strength::Tertiary,
        });
        options.case_level =
            Some(if sensitivity.case && !sensitivity.diacritics { CaseLevel::On } else { CaseLevel::Off });
        let collator = icu_collator::Collator::try_new(prefs, options).map_err(|error| error.to_string())?;
        Ok(Collator { collator, resolved_locale: resolved.to_string() })
    }

    pub(crate) fn compare(&self, lhs: &str, rhs: &str) -> Ordering {
        self.collator.compare(lhs, rhs)
    }

    pub(crate) fn resolved_locale(&self) -> &str {
        &self.resolved_locale
    }
}

/// Rounds `number` half to even to the fraction limits. NaN and the
/// infinities have no digits to round.
fn round(number: f64, digits: FractionDigits, currency_digits: Option<u8>) -> Option<Decimal> {
    let (min, max) = digits.resolve(currency_digits);
    let mut decimal = Decimal::try_from_f64(number, FloatPrecision::RoundTrip).ok()?;
    decimal.round(-i16::from(max));
    decimal.trim_end();
    decimal.pad_end(-i16::from(min));
    Some(decimal)
}

/// The unsigned text of NaN or an infinity, as ICU's root locale writes it.
fn non_finite(number: f64) -> &'static str {
    if number.is_nan() { "NaN" } else { "∞" }
}

/// The display names of a unit, in its short width, from the first of
/// `markers` that has them.
type UnitNames = DataPayload<ErasedMarker<UnitsDisplayNames<'static>>>;

fn load_unit_names<M>(attributes: &DataMarkerAttributes, locale: LocalePreferences) -> Option<UnitNames>
where
    M: DataMarker<DataStruct = UnitsDisplayNames<'static>>,
    icu_experimental::provider::Baked: DataProvider<M>,
{
    let locale = M::make_locale(locale);
    icu_experimental::provider::Baked
        .load(silent_request(DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale)))
        .ok()
        .map(|response| response.payload.cast())
}

/// The short CLDR names of a simple unit, which `icu_experimental` keeps by
/// the unit's category and how widely it's used.
fn unit_names(unit: &str, locale: LocalePreferences) -> Option<UnitNames> {
    let attributes = format!("short-{unit}");
    let attributes = DataMarkerAttributes::try_from_str(&attributes).ok()?;
    [
        load_unit_names::<UnitsNamesLengthCoreV1>,
        load_unit_names::<UnitsNamesLengthExtendedV1>,
        load_unit_names::<UnitsNamesLengthOutlierV1>,
        load_unit_names::<UnitsNamesAreaCoreV1>,
        load_unit_names::<UnitsNamesAreaExtendedV1>,
        load_unit_names::<UnitsNamesAreaOutlierV1>,
        load_unit_names::<UnitsNamesVolumeCoreV1>,
        load_unit_names::<UnitsNamesVolumeExtendedV1>,
        load_unit_names::<UnitsNamesVolumeOutlierV1>,
        load_unit_names::<UnitsNamesMassCoreV1>,
        load_unit_names::<UnitsNamesMassExtendedV1>,
        load_unit_names::<UnitsNamesMassOutlierV1>,
        load_unit_names::<UnitsNamesDurationCoreV1>,
        load_unit_names::<UnitsNamesDurationExtendedV1>,
        load_unit_names::<UnitsNamesDurationOutlierV1>,
    ]
    .into_iter()
    .find_map(|load| load(attributes, locale))
}

/// Formats a measure: `percent` with CLDR's percent pattern, which takes the
/// number as a percentage, and another unit with its short CLDR name in the
/// plural form of the rounded number.
fn format_unit(
    number: f64,
    sign: Sign,
    unit: Unit,
    decimal: Option<&Decimal>,
    locale: &Locale,
    formatter: &DecimalFormatter,
) -> Result<String, String> {
    let prefs = DecimalFormatterPreferences::from(locale);
    if unit == (Unit { unit: "percent", per: None }) {
        let locale = DecimalPercentV1::make_locale(prefs.locale_preferences);
        let essentials: DataResponse<DecimalPercentV1> = icu_experimental::provider::Baked
            .load(DataRequest { id: DataIdentifierBorrowed::for_locale(&locale), ..DataRequest::default() })
            .map_err(|error| error.to_string())?;
        let essentials = essentials.payload.get();
        let amount = match decimal {
            Some(decimal) => {
                AbstractFormatter::format_unsigned(formatter, decimal.absolute.clone()).write_to_string().into_owned()
            }
            None => non_finite(number).to_owned(),
        };
        let formatted = if sign == Sign::Negative {
            essentials.signed_pattern.interpolate((amount, &essentials.minus_sign)).write_to_string().into_owned()
        } else {
            essentials.unsigned_pattern.interpolate([amount]).write_to_string().into_owned()
        };
        return Ok(formatted);
    }

    let names = unit.per.is_none().then(|| unit_names(unit.unit, prefs.locale_preferences)).flatten();
    let names = names.ok_or_else(|| format!("{unit} can't be formatted on this platform"))?;
    let names = names.get();
    let (pattern, amount) = match decimal {
        Some(decimal) => {
            let rules = PluralRules::try_new_cardinal(PluralRulesPreferences::from(locale))
                .map_err(|error| error.to_string())?;
            (names.get(decimal.into(), &rules), formatter.format(decimal).write_to_string().into_owned())
        }
        // NaN and the infinities take the other plural form, as in ICU.
        None => (
            names.elements.get_default().1,
            AbstractFormatter::format_sign(formatter, non_finite(number), sign).write_to_string().into_owned(),
        ),
    };
    Ok(pattern.interpolate((amount,)).write_to_string().into_owned())
}

/// Formats with `icu_decimal`'s symbols and grouping for the locale. A currency
/// takes its CLDR pattern and symbol from `icu_experimental`, as does a unit.
pub(crate) fn format_number(
    number: f64,
    locale: &str,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<String, String> {
    let locale = resolve_locale(locale, Category::Numeric);
    let prefs = DecimalFormatterPreferences::from(&locale);
    let formatter =
        DecimalFormatter::try_new(prefs, DecimalFormatterOptions::default()).map_err(|error| error.to_string())?;
    // Negative zero keeps its sign, as in ICU and JavaScript's Intl.
    let sign = if number.is_sign_negative() && !number.is_nan() { Sign::Negative } else { Sign::None };
    let currency = match style {
        Style::Currency(currency) => currency,
        Style::Unit(unit) => {
            return format_unit(number, sign, unit, round(number, digits, None).as_ref(), &locale, &formatter);
        }
        Style::Decimal => {
            let formatted = match round(number, digits, None) {
                Some(decimal) => formatter.format(&decimal).write_to_string().into_owned(),
                None => {
                    AbstractFormatter::format_sign(&formatter, non_finite(number), sign).write_to_string().into_owned()
                }
            };
            return Ok(formatted);
        }
    };

    let currency = CurrencyType::try_from_str(currency).map_err(|error| error.to_string())?;
    let data = icu_experimental::provider::Baked;
    let fractions: DataResponse<CurrencyFractionsV1> =
        data.load(DataRequest::default()).map_err(|error| error.to_string())?;
    let currency_digits = fractions.payload.get().resolve(currency).digits;
    let amount = match round(number, digits, Some(currency_digits)) {
        Some(decimal) => {
            AbstractFormatter::format_unsigned(&formatter, decimal.absolute).write_to_string().into_owned()
        }
        None => non_finite(number).to_owned(),
    };

    // ICU4X's currency formatter always rounds to the currency's digits, so
    // this applies its CLDR pattern to the rounded number instead.
    let locale = CurrencyEssentialsV1::make_locale(prefs.locale_preferences);
    let essentials: DataResponse<CurrencyEssentialsV1> = prefs
        .nu_id(&locale)
        .into_iter()
        .chain([DataIdentifierBorrowed::for_locale(&locale)])
        .find_map(|id| data.load(silent_request(id)).ok())
        .ok_or("No currency patterns for the locale")?;
    let symbol: Option<DataResponse<CurrencySymbolsV1>> = data
        .load(silent_request(DataIdentifierBorrowed::for_marker_attributes_and_locale(
            CurrencySymbolsV1::make_attributes(currency, CurrencySymbolsV1::SHORT, &mut <_>::default()),
            &locale,
        )))
        .allow_identifier_not_found()
        .map_err(|error| error.to_string())?;
    let code = currency.iso_code();
    let symbol = symbol.as_ref().map(|symbol| symbol.payload.get());
    let (symbol, starts_with_letter, ends_with_letter) = symbol.map_or((code.as_str(), true, true), |symbol| {
        (symbol.as_str(), symbol.starts_with_letter(), symbol.ends_with_letter())
    });
    let essentials = essentials.payload.get();
    let negative =
        (sign == Sign::Negative).then(|| essentials.get_negative(starts_with_letter, ends_with_letter)).flatten();
    // An explicit negative pattern carries the sign itself.
    let (pattern, sign) = match negative {
        Some(pattern) => (pattern, Sign::None),
        None => (essentials.get_positive(starts_with_letter, ends_with_letter), sign),
    };
    Ok(AbstractFormatter::format_sign(&formatter, pattern.interpolate((amount, symbol)), sign)
        .write_to_string()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_locale_follows_posix_precedence() {
        let locale = |variables: &[(&str, &str)]| {
            environment_locale("LC_COLLATE", |name| {
                variables.iter().find(|(variable, _)| *variable == name).map(|(_, value)| (*value).to_owned())
            })
            .to_string()
        };
        assert_eq!(locale(&[]), "en-US");
        assert_eq!(locale(&[("LANG", "de_DE.UTF-8")]), "de-DE");
        assert_eq!(locale(&[("LC_COLLATE", "sv_SE"), ("LANG", "de_DE")]), "sv-SE");
        assert_eq!(locale(&[("LC_ALL", ""), ("LC_COLLATE", "sv_SE@euro")]), "sv-SE");
        assert_eq!(locale(&[("LC_ALL", "C.UTF-8"), ("LANG", "de_DE")]), "en-US");
        assert_eq!(locale(&[("LC_ALL", "POSIX")]), "en-US");
        assert_eq!(locale(&[("LC_ALL", "xx_ZZ"), ("LANG", "de_DE")]), "en-US");
        assert_eq!(locale(&[("LANG", "iw_IL")]), "he-IL");
        assert_eq!(locale(&[("LANG", "sr_RS.UTF-8@latin")]), "sr-Latn-RS");
    }

    #[test]
    fn unsupported_languages_take_the_default_language() {
        let default = Category::Collate.default_locale().id.to_string();
        assert_eq!(resolve_locale("sv-SE", Category::Collate).to_string(), "sv-SE");
        assert_eq!(resolve_locale("iw", Category::Collate).to_string(), "he");
        assert_eq!(resolve_locale("", Category::Collate).to_string(), default);
        assert_eq!(resolve_locale("xx-ZZ", Category::Collate).to_string(), default);
        assert_eq!(resolve_locale("xx-u-kn", Category::Collate).to_string(), format!("{default}-u-kn"));
        assert_eq!(resolve_locale("not a tag", Category::Collate).to_string(), default);
    }
}
