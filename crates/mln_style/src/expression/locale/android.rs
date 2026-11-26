//! The locale services on Android: the framework's ICU (`android.icu`, API
//! level 24), with the system's locale data, through JNI in the app's Java VM.
//! The default locale is the app's (`ULocale.getDefault()`).
//!
//! The Java VM is [`crate::android::java_vm`]'s. Without one, a collator
//! or number format fails. Each thread that evaluates an expression attaches
//! to the VM and stays attached.
//!
//! A collator honors `co`, `kf` and `kn`, though `co` only when ICU has that
//! collation type for the language. Its resolved locale is the language tag
//! it was asked for, as ICU4X's is.
//!
//! A number formats with `NumberFormatter` from API level 30. Before then, it
//! formats with `NumberFormat`, and a unit with `MeasureFormat` in its short
//! width over that. A compound unit needs API level 26 there. A few units are
//! newer than API level 24, so `mile-scandinavian` needs 26, and `percent` with
//! `NumberFormatter` and `petabyte` need 30. A unit the system's ICU lacks
//! fails to format.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use jni::errors::Error as JniError;
use jni::objects::{Global, JObject, JObjectArray, JString, JValue};
use jni::strings::{JNIStr, JNIString};
use jni::{Env, jni_sig, jni_str};
use parking_lot::Mutex;

use super::keywords::CollationKeywords;
use super::{FractionDigits, Sensitivity, Style, Unit, default_locale_override};
use crate::hash::HashSet;

/// The first API level with `android.icu.number.NumberFormatter`.
const NUMBER_FORMATTER_LEVEL: i32 = 30;

/// The first API level with `MeasureFormat.formatMeasurePerUnit`.
const PER_UNIT_LEVEL: i32 = 26;

/// Why a call into Android's ICU failed.
enum Failure {
    Jni(JniError),
    Unsupported(String),
}

impl From<JniError> for Failure {
    fn from(error: JniError) -> Self {
        Failure::Jni(error)
    }
}

impl Failure {
    fn into_message(self) -> String {
        match self {
            Failure::Jni(JniError::CaughtJavaException { name, msg, .. }) => format!("{name}: {msg}"),
            Failure::Jni(error) => error.to_string(),
            Failure::Unsupported(message) => message,
        }
    }
}

/// Runs `f` on this thread attached to the app's Java VM, in a frame of local
/// references of its own. A Java exception that `f` leaves fails it.
fn with_java<T>(f: impl FnOnce(&mut Env<'_>) -> Result<T, Failure>) -> Result<T, String> {
    let vm = crate::android::java_vm().ok_or_else(|| "Locale services need Android initialization".to_owned())?;
    vm.attach_current_thread(f).map_err(Failure::into_message)
}

/// The text of a Java string.
fn string(env: &Env<'_>, object: JObject<'_>) -> Result<String, JniError> {
    env.cast_local::<JString>(object)?.try_to_string(env)
}

/// The device's API level.
fn api_level(env: &mut Env<'_>) -> Result<i32, JniError> {
    static LEVEL: OnceLock<i32> = OnceLock::new();
    if let Some(level) = LEVEL.get() {
        return Ok(*level);
    }
    let level = env.get_static_field(jni_str!("android/os/Build$VERSION"), jni_str!("SDK_INT"), jni_sig!("I"))?.i()?;
    Ok(*LEVEL.get_or_init(|| level))
}

fn for_language_tag<'local>(env: &mut Env<'local>, tag: &str) -> Result<JObject<'local>, JniError> {
    let tag = env.new_string(tag)?;
    env.call_static_method(
        jni_str!("android/icu/util/ULocale"),
        jni_str!("forLanguageTag"),
        jni_sig!("(Ljava/lang/String;)Landroid/icu/util/ULocale;"),
        &[JValue::from(&tag)],
    )?
    .l()
}

fn language(env: &mut Env<'_>, locale: &JObject<'_>) -> Result<String, JniError> {
    let language = env.call_method(locale, jni_str!("getLanguage"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
    string(env, language)
}

/// The languages the system's ICU has locale data for.
fn available_languages(env: &mut Env<'_>) -> Result<&'static HashSet<String>, JniError> {
    static LANGUAGES: OnceLock<HashSet<String>> = OnceLock::new();
    if let Some(languages) = LANGUAGES.get() {
        return Ok(languages);
    }
    let locales = env
        .call_static_method(
            jni_str!("android/icu/util/ULocale"),
            jni_str!("getAvailableLocales"),
            jni_sig!("()[Landroid/icu/util/ULocale;"),
            &[],
        )?
        .l()?;
    let locales = env.cast_local::<JObjectArray>(locales)?;
    let mut languages = HashSet::default();
    for index in 0..locales.len(env)? {
        let language = env.with_local_frame(4, |env| -> Result<_, JniError> {
            let locale = locales.get_element(env, index)?;
            language(env, &locale)
        })?;
        languages.insert(language);
    }
    Ok(LANGUAGES.get_or_init(|| languages))
}

/// The locale a BCP 47 tag names, with its Unicode extensions, unless the tag
/// is empty or malformed or its language has no data.
fn named_locale<'local>(env: &mut Env<'local>, tag: &str) -> Result<Option<JObject<'local>>, JniError> {
    if tag.is_empty() {
        return Ok(None);
    }
    let locale = for_language_tag(env, tag)?;
    let language = language(env, &locale)?;
    Ok((!language.is_empty() && available_languages(env)?.contains(&language)).then_some(locale))
}

/// The locale a BCP 47 tag names, or the default locale when it names none.
fn resolve_locale<'local>(env: &mut Env<'local>, tag: &str) -> Result<JObject<'local>, JniError> {
    for tag in [Some(tag), default_locale_override()].into_iter().flatten() {
        if let Some(locale) = named_locale(env, tag)? {
            return Ok(locale);
        }
    }
    env.call_static_method(
        jni_str!("android/icu/util/ULocale"),
        jni_str!("getDefault"),
        jni_sig!("()Landroid/icu/util/ULocale;"),
        &[],
    )?
    .l()
}

/// A locale's language, script and region as a language tag, without its
/// keywords.
fn base_tag(env: &mut Env<'_>, locale: &JObject<'_>) -> Result<String, JniError> {
    let base = env.call_method(locale, jni_str!("getBaseName"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
    let base = env.new_object(
        jni_str!("android/icu/util/ULocale"),
        jni_sig!("(Ljava/lang/String;)V"),
        &[JValue::from(&base)],
    )?;
    let tag = env.call_method(&base, jni_str!("toLanguageTag"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
    string(env, tag)
}

/// Whether ICU has the collation type that `locale` requests (its
/// `collation` keyword, by ICU's name, such as `phonebook` for `phonebk`) for
/// its language.
fn has_collation_type(env: &mut Env<'_>, locale: &JObject<'_>) -> Result<bool, JniError> {
    let keyword = env.new_string("collation")?;
    let requested = env
        .call_method(
            locale,
            jni_str!("getKeywordValue"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::from(&keyword)],
        )?
        .l()?;
    if requested.is_null() {
        return Ok(false);
    }
    let requested = string(env, requested)?;
    let available = env
        .call_static_method(
            jni_str!("android/icu/text/Collator"),
            jni_str!("getKeywordValuesForLocale"),
            jni_sig!("(Ljava/lang/String;Landroid/icu/util/ULocale;Z)[Ljava/lang/String;"),
            &[JValue::from(&keyword), JValue::from(locale), JValue::Bool(false)],
        )?
        .l()?;
    let available = env.cast_local::<JObjectArray<JString>>(available)?;
    for index in 0..available.len(env)? {
        let value = available.get_element(env, index)?;
        if value.try_to_string(env)? == requested {
            return Ok(true);
        }
        env.delete_local_ref(value);
    }
    Ok(false)
}

/// An ICU collator, frozen so that threads can share it. Case without
/// diacritics is primary strength with the case level on.
pub(crate) struct Collator {
    collator: Global<JObject<'static>>,
    resolved_locale: String,
}

impl Collator {
    pub(crate) fn new(tag: &str, sensitivity: Sensitivity) -> Result<Self, String> {
        with_java(|env| {
            let resolved = resolve_locale(env, tag)?;
            let base = base_tag(env, &resolved)?;
            let mut keywords = CollationKeywords::parse(tag);
            let locale = for_language_tag(env, &keywords.append_to(&base))?;
            if !has_collation_type(env, &locale)? {
                keywords.collation = None;
            }

            let collator = env
                .call_static_method(
                    jni_str!("android/icu/text/Collator"),
                    jni_str!("getInstance"),
                    jni_sig!("(Landroid/icu/util/ULocale;)Landroid/icu/text/Collator;"),
                    &[JValue::from(&locale)],
                )?
                .l()?;
            // Collator's PRIMARY, SECONDARY and TERTIARY.
            let strength = match (sensitivity.diacritics, sensitivity.case) {
                (false, _) => 0,
                (true, false) => 1,
                (true, true) => 2,
            };
            env.call_method(&collator, jni_str!("setStrength"), jni_sig!("(I)V"), &[JValue::Int(strength)])?;
            if sensitivity.case
                && !sensitivity.diacritics
                && env.is_instance_of(&collator, jni_str!("android/icu/text/RuleBasedCollator"))?
            {
                env.call_method(&collator, jni_str!("setCaseLevel"), jni_sig!("(Z)V"), &[JValue::Bool(true)])?;
            }
            let collator =
                env.call_method(&collator, jni_str!("freeze"), jni_sig!("()Landroid/icu/text/Collator;"), &[])?.l()?;
            Ok(Collator { collator: env.new_global_ref(&collator)?, resolved_locale: keywords.append_to(&base) })
        })
    }

    pub(crate) fn compare(&self, lhs: &str, rhs: &str) -> Ordering {
        // Only a JNI failure, such as running out of memory, fails this.
        with_java(|env| {
            let (lhs, rhs) = (env.new_string(lhs)?, env.new_string(rhs)?);
            let order = env
                .call_method(
                    &self.collator,
                    jni_str!("compare"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;)I"),
                    &[JValue::from(&lhs), JValue::from(&rhs)],
                )?
                .i()?;
            Ok(order.cmp(&0))
        })
        .unwrap_or(Ordering::Equal)
    }

    pub(crate) fn resolved_locale(&self) -> &str {
        &self.resolved_locale
    }
}

/// The `MeasureUnit` constant of a simple unit, such as `FLUID_OUNCE` for
/// `fluid-ounce`, if the system's ICU has it.
fn measure_unit<'local>(env: &mut Env<'local>, unit: Unit, simple: &str) -> Result<JObject<'local>, Failure> {
    let name = JNIString::new(simple.to_ascii_uppercase().replace('-', "_"));
    let class = jni_str!("android/icu/util/MeasureUnit");
    // The durations of a day and longer, and of a second to an hour, are
    // `TimeUnit`s.
    let field = if matches!(simple, "day" | "hour" | "minute" | "month" | "second" | "week" | "year") {
        env.get_static_field(class, &name, jni_sig!("Landroid/icu/util/TimeUnit;"))
    } else {
        env.get_static_field(class, &name, jni_sig!("Landroid/icu/util/MeasureUnit;"))
    };
    field.and_then(jni::JValueOwned::l).map_err(|_| {
        env.exception_clear();
        Failure::Unsupported(format!("{unit} can't be formatted on this platform"))
    })
}

fn currency<'local>(env: &mut Env<'local>, code: &str) -> Result<JObject<'local>, JniError> {
    let code = env.new_string(code)?;
    env.call_static_method(
        jni_str!("android/icu/util/Currency"),
        jni_str!("getInstance"),
        jni_sig!("(Ljava/lang/String;)Landroid/icu/util/Currency;"),
        &[JValue::from(&code)],
    )?
    .l()
}

/// The currency's ISO 4217 fraction digits.
fn currency_digits(env: &mut Env<'_>, currency: &JObject<'_>) -> Result<u8, JniError> {
    let digits = env.call_method(currency, jni_str!("getDefaultFractionDigits"), jni_sig!("()I"), &[])?.i()?;
    Ok(u8::try_from(digits).unwrap_or(2))
}

/// `formatter` with its unit, or the unit it's per, set by `setter`.
fn with_unit<'local>(
    env: &mut Env<'local>,
    formatter: &JObject<'_>,
    setter: &JNIStr,
    unit: &JObject<'_>,
) -> Result<JObject<'local>, JniError> {
    env.call_method(
        formatter,
        setter,
        jni_sig!("(Landroid/icu/util/MeasureUnit;)Landroid/icu/number/NumberFormatterSettings;"),
        &[JValue::from(unit)],
    )?
    .l()
}

/// What a cached formatter formats with: the resolved locale, as a language
/// tag, and the style and fraction limits.
#[derive(PartialEq, Eq)]
struct FormatterKey {
    locale: String,
    currency: Option<String>,
    unit: Option<Unit>,
    digits: FractionDigits,
}

/// How many formatters [`formatter`] keeps. A style has few number formats,
/// each of which formats many numbers.
const CACHED_FORMATTERS: usize = 16;

/// A number formatter for a locale, style and fraction limits.
enum Formatter {
    /// A `LocalizedNumberFormatter` (API level 30), which is immutable and
    /// thread-safe.
    NumberFormatter(Global<JObject<'static>>),
    /// A `NumberFormat`, or a `MeasureFormat` over one, which aren't
    /// thread-safe.
    NumberFormat(Mutex<NumberFormat>),
}

/// A `NumberFormat`, or a `MeasureFormat` and the units it formats in.
struct NumberFormat {
    format: Global<JObject<'static>>,
    /// For a `MeasureFormat`: the unit, and the one it's per.
    units: Option<(Global<JObject<'static>>, Option<Global<JObject<'static>>>)>,
}

/// The formatter for the locale, style and fraction limits. The last ones
/// made are kept: making one takes far longer than formatting a number, and
/// ICU compiles a `NumberFormatter` only once it has formatted a few.
fn formatter(
    env: &mut Env<'_>,
    locale: &JObject<'_>,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<Arc<Formatter>, Failure> {
    static FORMATTERS: Mutex<VecDeque<(FormatterKey, Arc<Formatter>)>> = Mutex::new(VecDeque::new());
    let tag = env.call_method(locale, jni_str!("toLanguageTag"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
    let key = FormatterKey {
        locale: string(env, tag)?,
        currency: match style {
            Style::Currency(code) => Some(code.to_owned()),
            _ => None,
        },
        unit: match style {
            Style::Unit(unit) => Some(unit),
            _ => None,
        },
        digits,
    };
    if let Some((_, formatter)) = FORMATTERS.lock().iter().find(|(cached, _)| *cached == key) {
        return Ok(Arc::clone(formatter));
    }

    let formatter = Arc::new(if api_level(env)? >= NUMBER_FORMATTER_LEVEL {
        Formatter::NumberFormatter(number_formatter(env, locale, style, digits)?)
    } else {
        Formatter::NumberFormat(Mutex::new(number_format(env, locale, style, digits)?))
    });
    let mut formatters = FORMATTERS.lock();
    if formatters.len() == CACHED_FORMATTERS {
        formatters.pop_back();
    }
    formatters.push_front((key, Arc::clone(&formatter)));
    Ok(formatter)
}

/// A `LocalizedNumberFormatter`, whose units take their short width by
/// default.
fn number_formatter(
    env: &mut Env<'_>,
    locale: &JObject<'_>,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<Global<JObject<'static>>, Failure> {
    let mut formatter = env
        .call_static_method(
            jni_str!("android/icu/number/NumberFormatter"),
            jni_str!("withLocale"),
            jni_sig!("(Landroid/icu/util/ULocale;)Landroid/icu/number/LocalizedNumberFormatter;"),
            &[JValue::from(locale)],
        )?
        .l()?;
    let currency_digits = match style {
        Style::Decimal => None,
        Style::Currency(code) => {
            let currency = currency(env, code)?;
            formatter = with_unit(env, &formatter, jni_str!("unit"), &currency)?;
            Some(currency_digits(env, &currency)?)
        }
        // As in Intl, a number in percent is already a percentage, which ICU's
        // percent unit doesn't multiply.
        Style::Unit(unit) => {
            let simple = measure_unit(env, unit, unit.unit)?;
            formatter = with_unit(env, &formatter, jni_str!("unit"), &simple)?;
            if let Some(per) = unit.per {
                let per = measure_unit(env, unit, per)?;
                formatter = with_unit(env, &formatter, jni_str!("perUnit"), &per)?;
            }
            None
        }
    };

    let (min, max) = digits.resolve(currency_digits);
    let precision = env
        .call_static_method(
            jni_str!("android/icu/number/Precision"),
            jni_str!("minMaxFraction"),
            jni_sig!("(II)Landroid/icu/number/FractionPrecision;"),
            &[JValue::Int(min.into()), JValue::Int(max.into())],
        )?
        .l()?;
    let formatter = env
        .call_method(
            &formatter,
            jni_str!("precision"),
            jni_sig!("(Landroid/icu/number/Precision;)Landroid/icu/number/NumberFormatterSettings;"),
            &[JValue::from(&precision)],
        )?
        .l()?;
    let half_even = env
        .get_static_field(
            jni_str!("java/math/RoundingMode"),
            jni_str!("HALF_EVEN"),
            jni_sig!("Ljava/math/RoundingMode;"),
        )?
        .l()?;
    let formatter = env
        .call_method(
            &formatter,
            jni_str!("roundingMode"),
            jni_sig!("(Ljava/math/RoundingMode;)Landroid/icu/number/NumberFormatterSettings;"),
            &[JValue::from(&half_even)],
        )?
        .l()?;
    Ok(env.new_global_ref(&formatter)?)
}

/// A `NumberFormat` in the decimal, currency or percent style, or a
/// `MeasureFormat` in its short width over one in the decimal style.
fn number_format(
    env: &mut Env<'_>,
    locale: &JObject<'_>,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<NumberFormat, Failure> {
    let class = jni_str!("android/icu/text/NumberFormat");
    let instance_sig = jni_sig!("(Landroid/icu/util/ULocale;)Landroid/icu/text/NumberFormat;");
    let locale_arg = [JValue::from(locale)];
    let mut units = None;
    let (format, currency_digits) = match style {
        Style::Decimal => {
            (env.call_static_method(class, jni_str!("getInstance"), instance_sig, &locale_arg)?.l()?, None)
        }
        Style::Currency(code) => {
            let format =
                env.call_static_method(class, jni_str!("getCurrencyInstance"), instance_sig, &locale_arg)?.l()?;
            let currency = currency(env, code)?;
            env.call_method(
                &format,
                jni_str!("setCurrency"),
                jni_sig!("(Landroid/icu/util/Currency;)V"),
                &[JValue::from(&currency)],
            )?;
            (format, Some(currency_digits(env, &currency)?))
        }
        // As in Intl, a number in percent is already a percentage.
        Style::Unit(Unit { unit: "percent", per: None }) => {
            let format =
                env.call_static_method(class, jni_str!("getPercentInstance"), instance_sig, &locale_arg)?.l()?;
            if env.is_instance_of(&format, jni_str!("android/icu/text/DecimalFormat"))? {
                env.call_method(&format, jni_str!("setMultiplier"), jni_sig!("(I)V"), &[JValue::Int(1)])?;
            }
            (format, None)
        }
        Style::Unit(unit) => {
            if unit.per.is_some() && api_level(env)? < PER_UNIT_LEVEL {
                return Err(Failure::Unsupported(format!("{unit} can't be formatted on this platform")));
            }
            let simple = measure_unit(env, unit, unit.unit)?;
            let per = unit.per.map(|per| measure_unit(env, unit, per)).transpose()?;
            let per = per.map(|per| env.new_global_ref(&per)).transpose()?;
            units = Some((env.new_global_ref(&simple)?, per));
            (env.call_static_method(class, jni_str!("getInstance"), instance_sig, &locale_arg)?.l()?, None)
        }
    };

    let (min, max) = digits.resolve(currency_digits);
    env.call_method(&format, jni_str!("setMaximumFractionDigits"), jni_sig!("(I)V"), &[JValue::Int(max.into())])?;
    env.call_method(&format, jni_str!("setMinimumFractionDigits"), jni_sig!("(I)V"), &[JValue::Int(min.into())])?;
    // BigDecimal.ROUND_HALF_EVEN.
    env.call_method(&format, jni_str!("setRoundingMode"), jni_sig!("(I)V"), &[JValue::Int(6)])?;
    if units.is_none() {
        return Ok(NumberFormat { format: env.new_global_ref(&format)?, units });
    }

    let width = env
        .get_static_field(
            jni_str!("android/icu/text/MeasureFormat$FormatWidth"),
            jni_str!("SHORT"),
            jni_sig!("Landroid/icu/text/MeasureFormat$FormatWidth;"),
        )?
        .l()?;
    let measure_format = env
        .call_static_method(
            jni_str!("android/icu/text/MeasureFormat"),
            jni_str!("getInstance"),
            jni_sig!(
                "(Landroid/icu/util/ULocale;Landroid/icu/text/MeasureFormat$FormatWidth;Landroid/icu/text/NumberFormat;)Landroid/icu/text/MeasureFormat;"
            ),
            &[JValue::from(locale), JValue::from(&width), JValue::from(&format)],
        )?
        .l()?;
    Ok(NumberFormat { format: env.new_global_ref(&measure_format)?, units })
}

/// Formats `number` with `formatter`.
fn format(env: &mut Env<'_>, formatter: &Formatter, number: f64) -> Result<String, JniError> {
    let format = match formatter {
        Formatter::NumberFormatter(formatter) => {
            let formatted = env
                .call_method(
                    formatter,
                    jni_str!("format"),
                    jni_sig!("(D)Landroid/icu/number/FormattedNumber;"),
                    &[JValue::Double(number)],
                )?
                .l()?;
            let formatted =
                env.call_method(&formatted, jni_str!("toString"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
            return string(env, formatted);
        }
        Formatter::NumberFormat(format) => format.lock(),
    };
    let Some((unit, per)) = &format.units else {
        let formatted = env
            .call_method(
                &format.format,
                jni_str!("format"),
                jni_sig!("(D)Ljava/lang/String;"),
                &[JValue::Double(number)],
            )?
            .l()?;
        return string(env, formatted);
    };
    // Local references, which the arguments below take.
    let unit = env.new_local_ref(unit)?;
    let per = per.as_ref().map(|per| env.new_local_ref(per)).transpose()?;
    let amount = env
        .call_static_method(
            jni_str!("java/lang/Double"),
            jni_str!("valueOf"),
            jni_sig!("(D)Ljava/lang/Double;"),
            &[JValue::Double(number)],
        )?
        .l()?;
    let measure = env.new_object(
        jni_str!("android/icu/util/Measure"),
        jni_sig!("(Ljava/lang/Number;Landroid/icu/util/MeasureUnit;)V"),
        &[JValue::from(&amount), JValue::from(&unit)],
    )?;
    let formatted = match per {
        None => env
            .call_method(
                &format.format,
                jni_str!("format"),
                jni_sig!("(Ljava/lang/Object;)Ljava/lang/String;"),
                &[JValue::from(&measure)],
            )?
            .l()?,
        Some(per) => {
            let builder = env.new_object(jni_str!("java/lang/StringBuilder"), jni_sig!("()V"), &[])?;
            let position = env.new_object(jni_str!("java/text/FieldPosition"), jni_sig!("(I)V"), &[JValue::Int(0)])?;
            let builder = env
                .call_method(
                    &format.format,
                    jni_str!("formatMeasurePerUnit"),
                    jni_sig!(
                        "(Landroid/icu/util/Measure;Landroid/icu/util/MeasureUnit;Ljava/lang/StringBuilder;Ljava/text/FieldPosition;)Ljava/lang/StringBuilder;"
                    ),
                    &[JValue::from(&measure), JValue::from(&per), JValue::from(&builder), JValue::from(&position)],
                )?
                .l()?;
            env.call_method(&builder, jni_str!("toString"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?
        }
    };
    string(env, formatted)
}

/// Formats with `NumberFormatter`, or before API level 30 with
/// `NumberFormat` and `MeasureFormat`.
pub(crate) fn format_number(
    number: f64,
    tag: &str,
    style: Style<'_>,
    digits: FractionDigits,
) -> Result<String, String> {
    with_java(|env| {
        let locale = resolve_locale(env, tag)?;
        let formatter = formatter(env, &locale, style, digits)?;
        Ok(format(env, &formatter, number)?)
    })
}
