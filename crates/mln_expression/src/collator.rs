// Ports include/mln/style/expression/collator.hpp,
// src/mln/style/expression/collator.cpp and the default platform collator
// (platform/default/src/mln/i18n/collator.cpp).
//
// The default collator ignores the locale; it compares with nunicode's DUCET
// collation (`nu_strcoll` / `nu_strcasecoll`) and strips diacritics with
// nunicode's `unaccent` (//vendor/nunicode).

/// `mln::platform::Collator` (default implementation: ignores the locale).
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlatformCollator {
    case_sensitive: bool,
    diacritic_sensitive: bool,
}

impl PlatformCollator {
    fn compare(&self, lhs: &str, rhs: &str) -> i32 {
        nunicode::collator_compare(lhs, rhs, self.case_sensitive, self.diacritic_sensitive)
    }

    fn resolved_locale() -> String {
        String::new()
    }
}

/// `expression::Collator`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collator {
    collator: PlatformCollator,
}

impl Collator {
    #[must_use]
    pub fn new(case_sensitive: bool, diacritic_sensitive: bool, _locale: Option<String>) -> Self {
        Collator { collator: PlatformCollator { case_sensitive, diacritic_sensitive } }
    }

    #[must_use]
    pub fn compare(&self, lhs: &str, rhs: &str) -> i32 {
        self.collator.compare(lhs, rhs)
    }

    #[must_use]
    pub fn resolved_locale(&self) -> String {
        PlatformCollator::resolved_locale()
    }
}
