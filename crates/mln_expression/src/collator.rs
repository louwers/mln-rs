use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::locale::{self, Sensitivity};

/// Compares strings in a locale's order, with the sensitivity to case and
/// diacritics the expression asks for.
#[derive(Clone)]
pub struct Collator {
    collator: Arc<locale::Collator>,
    case_sensitive: bool,
    diacritic_sensitive: bool,
}

impl Collator {
    /// A collator for a BCP 47 language tag, or for the platform's default
    /// locale when the tag is absent, malformed or its language unsupported.
    /// The tag's `co` (collation type), `kf` (case first) and `kn` (numeric)
    /// Unicode extensions apply where the platform supports them.
    ///
    /// # Errors
    ///
    /// When the platform has no collation for the locale.
    pub(crate) fn new(case_sensitive: bool, diacritic_sensitive: bool, locale: Option<&str>) -> Result<Self, String> {
        let sensitivity = Sensitivity { case: case_sensitive, diacritics: diacritic_sensitive };
        let collator = locale::Collator::new(locale.unwrap_or_default(), sensitivity)?;
        Ok(Collator { collator: Arc::new(collator), case_sensitive, diacritic_sensitive })
    }

    #[must_use]
    pub(crate) fn compare(&self, lhs: &str, rhs: &str) -> Ordering {
        self.collator.compare(lhs, rhs)
    }

    /// The locale the collator compares in: the supported language it
    /// resolved to, with the Unicode extensions it honors.
    #[must_use]
    pub(crate) fn resolved_locale(&self) -> &str {
        self.collator.resolved_locale()
    }
}

impl PartialEq for Collator {
    fn eq(&self, other: &Self) -> bool {
        self.resolved_locale() == other.resolved_locale()
            && self.case_sensitive == other.case_sensitive
            && self.diacritic_sensitive == other.diacritic_sensitive
    }
}

impl Eq for Collator {}

impl fmt::Debug for Collator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Collator")
            .field("resolved_locale", &self.resolved_locale())
            .field("case_sensitive", &self.case_sensitive)
            .field("diacritic_sensitive", &self.diacritic_sensitive)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering::{Equal, Greater, Less};

    use super::Collator;

    fn collator(locale: &str, case_sensitive: bool, diacritic_sensitive: bool) -> Collator {
        Collator::new(case_sensitive, diacritic_sensitive, Some(locale)).expect("collator")
    }

    #[test]
    fn sensitivity_selects_what_tells_strings_apart() {
        for (case_sensitive, diacritic_sensitive) in [(false, false), (true, false), (false, true), (true, true)] {
            let c = collator("en-US", case_sensitive, diacritic_sensitive);
            assert_eq!(c.compare("A", "a") == Equal, !case_sensitive, "{c:?}");
            assert_eq!(c.compare("é", "e") == Equal, !diacritic_sensitive, "{c:?}");
            assert_eq!(c.compare("a", "b"), Less, "{c:?}");
        }
    }

    #[test]
    fn locale_and_extensions_select_the_order() {
        assert_eq!(collator("sv-SE", false, false).compare("ä", "z"), Greater);
        assert_eq!(collator("de-DE", false, false).compare("ä", "z"), Less);
        assert_eq!(collator("de-DE", false, false).compare("ä", "ae"), Less);
        assert_eq!(collator("en-US", false, false).compare("2", "10"), Greater);
        assert_eq!(collator("en-US-u-kn", false, false).compare("2", "10"), Less);
    }

    // Foundation's comparison has no collation types.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn collation_types_select_the_order() {
        assert_eq!(collator("de-DE-u-co-phonebk", false, false).compare("ä", "ae"), Equal);
    }

    #[test]
    fn resolved_locale_keeps_the_honored_extensions() {
        let default = Collator::new(false, false, None).expect("collator");
        // V8 resolves to the locale of its collation data, without the region
        // that doesn't change the order.
        let (swedish, german) = if cfg!(target_vendor = "apple") {
            ("sv-SE", "de-DE-u-kn")
        } else if cfg!(target_family = "wasm") {
            ("sv", "de-u-co-phonebk-kn")
        } else {
            ("sv-SE", "de-DE-u-co-phonebk-kn")
        };
        assert_eq!(collator("sv-SE", false, false).resolved_locale(), swedish);
        assert_eq!(collator("de-DE-u-co-phonebk-kn", false, false).resolved_locale(), german);
        assert_eq!(collator("en-US-u-co-phonebk", false, false).resolved_locale(), "en-US");
        assert_eq!(collator("en-u-ca-buddhist", false, false).resolved_locale(), "en");
        assert_eq!(collator("xx-ZZ", false, false).resolved_locale(), default.resolved_locale());
        assert_eq!(collator("xx-ZZ", false, false), default);
        assert_ne!(collator("xx-ZZ", true, false), default);
    }
}
