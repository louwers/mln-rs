//! The collation keywords of a language tag's Unicode extension (`-u-`), for
//! the platforms that take them as collator options.

use std::fmt::Write as _;

/// A collator's `kf` Unicode extension: which case sorts first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CaseFirst {
    Upper,
    Lower,
    Off,
}

impl CaseFirst {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CaseFirst::Upper => "upper",
            CaseFirst::Lower => "lower",
            CaseFirst::Off => "false",
        }
    }
}

/// The collation keywords of a BCP 47 language tag's Unicode extension, for
/// the platforms that take them as options rather than in the tag. A keyword
/// whose value isn't valid for it is left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct CollationKeywords {
    /// `co`, such as `phonebk`.
    pub(crate) collation: Option<String>,
    /// `kf`.
    pub(crate) case_first: Option<CaseFirst>,
    /// `kn`.
    pub(crate) numeric: Option<bool>,
}

impl CollationKeywords {
    pub(crate) fn parse(tag: &str) -> Self {
        let tag = tag.to_ascii_lowercase();
        let mut keywords = CollationKeywords::default();
        let mut in_unicode_extension = false;
        let mut keyword: Option<(&str, Vec<&str>)> = None;
        for subtag in tag.split(['-', '_']).chain([""]) {
            if subtag.len() == 2 && in_unicode_extension {
                if let Some((key, value)) = keyword.replace((subtag, Vec::new())) {
                    keywords.set(key, &value);
                }
            } else if subtag.len() > 2 {
                if let Some((_, value)) = keyword.as_mut() {
                    value.push(subtag);
                }
            } else {
                // A singleton, which starts an extension, or the end.
                if let Some((key, value)) = keyword.take() {
                    keywords.set(key, &value);
                }
                if subtag == "x" {
                    break;
                }
                in_unicode_extension = subtag == "u";
            }
        }
        keywords
    }

    /// Sets a keyword from its value's subtags, unless it's set already.
    fn set(&mut self, key: &str, value: &[&str]) {
        match (key, value) {
            ("co", [_, ..])
                if self.collation.is_none()
                    && value
                        .iter()
                        .all(|subtag| subtag.len() <= 8 && subtag.bytes().all(|b| b.is_ascii_alphanumeric())) =>
            {
                self.collation = Some(value.join("-"));
            }
            ("kf", [value]) if self.case_first.is_none() => {
                self.case_first = match *value {
                    "upper" => Some(CaseFirst::Upper),
                    "lower" => Some(CaseFirst::Lower),
                    "false" => Some(CaseFirst::Off),
                    _ => None,
                };
            }
            ("kn", [] | ["true"]) if self.numeric.is_none() => self.numeric = Some(true),
            ("kn", ["false"]) if self.numeric.is_none() => self.numeric = Some(false),
            _ => {}
        }
    }

    /// `base`, a language tag without extensions, with these keywords in
    /// canonical order.
    pub(crate) fn append_to(&self, base: &str) -> String {
        let mut tag = base.to_owned();
        if self.collation.is_none() && self.case_first.is_none() && self.numeric.is_none() {
            return tag;
        }
        tag.push_str("-u");
        if let Some(collation) = &self.collation {
            let _ = write!(tag, "-co-{collation}");
        }
        if let Some(case_first) = self.case_first {
            let _ = write!(tag, "-kf-{}", case_first.as_str());
        }
        match self.numeric {
            Some(true) => tag.push_str("-kn"),
            Some(false) => tag.push_str("-kn-false"),
            None => {}
        }
        tag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collation_keywords_come_from_the_unicode_extension() {
        let keywords = CollationKeywords::parse("de-DE-u-co-phonebk-kn");
        assert_eq!(keywords.collation.as_deref(), Some("phonebk"));
        assert_eq!(keywords.numeric, Some(true));
        assert_eq!(keywords.append_to("de-DE"), "de-DE-u-co-phonebk-kn");

        let keywords = CollationKeywords::parse("en-US-u-ca-buddhist-KF-Upper-kn-false-x-u-co-x");
        assert_eq!(keywords.append_to("en-US"), "en-US-u-kf-upper-kn-false");
        assert_eq!(CollationKeywords::parse("en-US-u-kn-true").append_to("en-US"), "en-US-u-kn");
        assert_eq!(CollationKeywords::parse("en-u-kf-sideways-kn-maybe").append_to("en"), "en");
        assert_eq!(CollationKeywords::parse("en-a-co-xyz-u-co-big5han").collation.as_deref(), Some("big5han"));
        assert_eq!(CollationKeywords::parse("en-co-phonebk"), CollationKeywords::default());
        assert_eq!(CollationKeywords::parse("not a tag"), CollationKeywords::default());
    }
}
