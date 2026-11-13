//! The locale services in the browser: its `Intl.Collator` and
//! `Intl.NumberFormat`, with the browser's locale data. The default locale is
//! the browser's.
//!
//! The collation keywords go to `Intl.Collator` as options, so they apply in
//! the default locale too; the resolved locale keeps those that the
//! collator's `resolvedOptions()` reports as applied. `Intl.NumberFormat`
//! rounds the double's exact binary value, so a decimal tie that the double
//! can't represent, such as 2.675, rounds by its nearest double (2.67), and
//! it takes at most 100 fraction digits.
//!
//! JavaScript objects stay on the thread that made them, so each thread keeps
//! its own `Intl` objects, made on first use.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

use super::keywords::CollationKeywords;
use super::{FractionDigits, Sensitivity};

mod intl {
    use js_sys::Object;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = Intl)]
        pub(super) type Collator;

        #[wasm_bindgen(constructor, js_namespace = Intl, catch)]
        pub(super) fn new(locales: &JsValue, options: &Object) -> Result<Collator, JsValue>;

        #[wasm_bindgen(method)]
        pub(super) fn compare(this: &Collator, lhs: &str, rhs: &str) -> f64;

        #[wasm_bindgen(method, js_name = resolvedOptions)]
        pub(super) fn resolved_options(this: &Collator) -> ResolvedCollatorOptions;

        pub(super) type ResolvedCollatorOptions;

        #[wasm_bindgen(method, getter)]
        pub(super) fn locale(this: &ResolvedCollatorOptions) -> String;

        #[wasm_bindgen(method, getter)]
        pub(super) fn collation(this: &ResolvedCollatorOptions) -> Option<String>;

        #[wasm_bindgen(method, getter, js_name = caseFirst)]
        pub(super) fn case_first(this: &ResolvedCollatorOptions) -> Option<String>;

        #[wasm_bindgen(method, getter)]
        pub(super) fn numeric(this: &ResolvedCollatorOptions) -> Option<bool>;
    }

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = Intl)]
        pub(super) type NumberFormat;

        #[wasm_bindgen(constructor, js_namespace = Intl, catch)]
        pub(super) fn new(locales: &JsValue, options: &Object) -> Result<NumberFormat, JsValue>;

        #[wasm_bindgen(method)]
        pub(super) fn format(this: &NumberFormat, number: f64) -> String;
    }
}

/// The most fraction digits `Intl.NumberFormat` takes.
const MAX_FRACTION_DIGITS: u8 = 100;

fn set(object: &Object, key: &str, value: impl Into<JsValue>) {
    // Setting a property of a plain object can't fail.
    let _ = Reflect::set(object, &JsValue::from_str(key), &value.into());
}

/// Makes an `Intl` object for a language tag, or for the browser's default
/// locale when `Intl` rejects the tag.
fn construct<T>(tag: &str, options: &Object, new: fn(&JsValue, &Object) -> Result<T, JsValue>) -> Result<T, String> {
    let requested = if tag.is_empty() { JsValue::UNDEFINED } else { JsValue::from_str(tag) };
    new(&requested, options)
        .or_else(|_| new(&JsValue::UNDEFINED, options))
        .map_err(|error| error.as_string().unwrap_or_else(|| format!("{error:?}")))
}

/// What makes an `Intl.Collator`.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CollatorSettings {
    tag: String,
    sensitivity: Sensitivity,
    keywords: CollationKeywords,
}

impl CollatorSettings {
    fn construct(&self) -> Result<intl::Collator, String> {
        let options = Object::new();
        set(&options, "usage", "sort");
        let sensitivity = match (self.sensitivity.diacritics, self.sensitivity.case) {
            (false, false) => "base",
            (false, true) => "case",
            (true, false) => "accent",
            (true, true) => "variant",
        };
        set(&options, "sensitivity", sensitivity);
        if let Some(collation) = &self.keywords.collation {
            set(&options, "collation", collation.as_str());
        }
        if let Some(case_first) = self.keywords.case_first {
            set(&options, "caseFirst", case_first.as_str());
        }
        if let Some(numeric) = self.keywords.numeric {
            set(&options, "numeric", numeric);
        }
        construct(&self.tag, &options, intl::Collator::new)
    }
}

thread_local! {
    static COLLATORS: RefCell<HashMap<CollatorSettings, intl::Collator>> = RefCell::new(HashMap::new());
    static NUMBER_FORMATS: RefCell<HashMap<NumberFormatSettings, intl::NumberFormat>> =
        RefCell::new(HashMap::new());
}

/// An `Intl.Collator`'s settings, with the collator in each thread's cache.
pub(crate) struct Collator {
    settings: CollatorSettings,
    resolved_locale: String,
}

impl Collator {
    pub(crate) fn new(tag: &str, sensitivity: Sensitivity) -> Result<Self, String> {
        let settings = CollatorSettings { tag: tag.to_owned(), sensitivity, keywords: CollationKeywords::parse(tag) };
        let collator = settings.construct()?;
        let resolved = collator.resolved_options();
        let requested = &settings.keywords;
        let honored = CollationKeywords {
            collation: requested.collation.clone().filter(|collation| resolved.collation().as_ref() == Some(collation)),
            case_first: requested
                .case_first
                .filter(|case_first| resolved.case_first().as_deref() == Some(case_first.as_str())),
            numeric: requested.numeric.filter(|numeric| resolved.numeric() == Some(*numeric)),
        };
        let locale = resolved.locale();
        let resolved_locale = honored.append_to(locale.split("-u-").next().unwrap_or_default());
        COLLATORS.with_borrow_mut(|collators| collators.insert(settings.clone(), collator));
        Ok(Collator { settings, resolved_locale })
    }

    pub(crate) fn compare(&self, lhs: &str, rhs: &str) -> Ordering {
        COLLATORS.with_borrow_mut(|collators| {
            let collator = match collators.entry(self.settings.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => match self.settings.construct() {
                    Ok(collator) => entry.insert(collator),
                    // Not in practice: `new` made one with these settings.
                    Err(_) => return lhs.cmp(rhs),
                },
            };
            collator.compare(lhs, rhs).partial_cmp(&0.0).unwrap_or(Ordering::Equal)
        })
    }

    pub(crate) fn resolved_locale(&self) -> &str {
        &self.resolved_locale
    }
}

/// What makes an `Intl.NumberFormat`.
#[derive(Clone, PartialEq, Eq, Hash)]
struct NumberFormatSettings {
    tag: String,
    currency: Option<String>,
    digits: FractionDigits,
}

impl NumberFormatSettings {
    fn construct(&self) -> Result<intl::NumberFormat, String> {
        let options = Object::new();
        match &self.currency {
            Some(currency) => {
                set(&options, "style", "currency");
                set(&options, "currency", currency.as_str());
            }
            None => set(&options, "style", "decimal"),
        }
        if let Some(min) = self.digits.min {
            set(&options, "minimumFractionDigits", min.min(MAX_FRACTION_DIGITS));
        }
        if let Some(max) = self.digits.max {
            set(&options, "maximumFractionDigits", max.min(MAX_FRACTION_DIGITS));
        }
        set(&options, "roundingMode", "halfEven");
        construct(&self.tag, &options, intl::NumberFormat::new)
    }
}

/// Formats with an `Intl.NumberFormat` in the decimal or currency style,
/// which applies the default fraction limits itself.
pub(crate) fn format_number(
    number: f64,
    tag: &str,
    currency: Option<&str>,
    digits: FractionDigits,
) -> Result<String, String> {
    let settings = NumberFormatSettings { tag: tag.to_owned(), currency: currency.map(str::to_owned), digits };
    NUMBER_FORMATS.with_borrow_mut(|formats| {
        let format = match formats.entry(settings) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let format = entry.key().construct()?;
                entry.insert(format)
            }
        };
        Ok(format.format(number))
    })
}
