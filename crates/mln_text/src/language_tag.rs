// Ports src/mln/text/language_tag.hpp and src/mln/text/language_tag.cpp.
//
// The C++ parses BCP 47 with a Boost.Spirit Qi grammar; this is the same
// grammar as a hand-written PEG with Qi's semantics: ordered choice (the
// first alternative that matches wins), greedy repetition that never gives
// characters back, ASCII character classes, and the semantic actions that
// store language/script/region when their sub-rule matched inside
// `langtag`. Invalid tags are treated as empty/"default".
//
// Quirk kept: the grammar wraps `start` in `no_case[...]`, but Qi
// directives do not reach into separately defined rules, so the literals
// ("x", the singletons a-w/y-z, the grandfathered tags) are case-sensitive
// (`//difftest/text:misc_difftest`: "ab-X-1" and "ab-U-51" are invalid).

/// `LanguageTag`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageTag {
    /// ISO 639
    pub language: Option<String>,
    /// ISO 15924
    pub script: Option<String>,
    /// ISO 3316-1 || UN M.49
    pub region: Option<String>,
}

struct Parser<'a> {
    s: &'a [u8],
    tag: LanguageTag,
}

fn alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}
fn digit(c: u8) -> bool {
    c.is_ascii_digit()
}
fn alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

impl Parser<'_> {
    fn at(&self, i: usize) -> Option<u8> {
        self.s.get(i).copied()
    }

    /// `repeat(min, max)[class]`, greedy; the end position on success.
    fn repeat(&self, i: usize, min: usize, max: usize, class: fn(u8) -> bool) -> Option<usize> {
        let mut n = 0;
        while n < max && self.at(i + n).is_some_and(class) {
            n += 1;
        }
        (n >= min).then_some(i + n)
    }

    /// `lit(c)`.
    fn lit(&self, i: usize, c: u8) -> Option<usize> {
        (self.at(i) == Some(c)).then_some(i + 1)
    }

    /// `lit("...")` (case-sensitive, see the file comment).
    fn lit_str(&self, i: usize, lit: &str) -> Option<usize> {
        let l = lit.as_bytes();
        let s = self.s.get(i..i + l.len())?;
        (s == l).then_some(i + l.len())
    }

    /// `(&lit('-') | eoi)`
    fn boundary(&self, i: usize) -> Option<usize> {
        match self.at(i) {
            None | Some(b'-') => Some(i),
            _ => None,
        }
    }

    /// `start %= no_case[langtag | privateuse | grandfathered]`
    fn start(&mut self, i: usize) -> Option<usize> {
        self.langtag(i).or_else(|| self.privateuse(i)).or_else(|| self.grandfathered(i))
    }

    /// ```text
    /// langtag %= language[...] >> -("-" >> script[...]) >> -("-" >> region[...])
    ///     >> *("-" >> variant) >> *("-" >> extension) >> -("-" >> privateuse)
    /// ```
    fn langtag(&mut self, i: usize) -> Option<usize> {
        let (mut i, language) = self.language(i)?;
        self.tag.language = Some(language);
        if let Some(j) = self.lit(i, b'-')
            && let Some((k, script)) = self.script(j)
        {
            self.tag.script = Some(script);
            i = k;
        }
        if let Some(j) = self.lit(i, b'-')
            && let Some((k, region)) = self.region(j)
        {
            self.tag.region = Some(region);
            i = k;
        }
        while let Some(j) = self.lit(i, b'-').and_then(|j| self.variant(j)) {
            i = j;
        }
        while let Some(j) = self.lit(i, b'-').and_then(|j| self.extension(j)) {
            i = j;
        }
        if let Some(j) = self.lit(i, b'-').and_then(|j| self.privateuse(j)) {
            i = j;
        }
        Some(i)
    }

    /// `language %= (repeat(2, 3)[alpha] >> -("-" >> extlang)) | repeat(4)[alpha]
    /// | repeat(5, 8)[alpha]`; the attribute is the alpha characters of the
    /// matching alternative (extlang has none).
    fn language(&self, i: usize) -> Option<(usize, String)> {
        let text = |end: usize| String::from_utf8_lossy(&self.s[i..end]).into_owned();
        if let Some(j) = self.repeat(i, 2, 3, alpha) {
            let value = text(j);
            let end = self.lit(j, b'-').and_then(|k| self.extlang(k)).unwrap_or(j);
            return Some((end, value));
        }
        if let Some(j) = self.repeat(i, 4, 4, alpha) {
            return Some((j, text(j)));
        }
        self.repeat(i, 5, 8, alpha).map(|j| (j, text(j)))
    }

    /// `extlang = repeat(3)[alpha] >> (&lit('-') | eoi) >> repeat(0, 2)["-" >>
    /// repeat(3)[alpha] >> (&lit('-') | eoi)]`
    fn extlang(&self, i: usize) -> Option<usize> {
        let mut i = self.repeat(i, 3, 3, alpha).and_then(|j| self.boundary(j))?;
        for _ in 0..2 {
            match self.lit(i, b'-').and_then(|j| self.repeat(j, 3, 3, alpha)).and_then(|j| self.boundary(j)) {
                Some(j) => i = j,
                None => break,
            }
        }
        Some(i)
    }

    /// `script = repeat(4)[alpha] >> (&lit('-') | eoi)`
    fn script(&self, i: usize) -> Option<(usize, String)> {
        let j = self.repeat(i, 4, 4, alpha)?;
        self.boundary(j)?;
        Some((j, String::from_utf8_lossy(&self.s[i..j]).into_owned()))
    }

    /// `region = (repeat(2)[alpha] | repeat(3)[digit]) >> (&lit('-') | eoi)`
    fn region(&self, i: usize) -> Option<(usize, String)> {
        let j = self.repeat(i, 2, 2, alpha).or_else(|| self.repeat(i, 3, 3, digit))?;
        self.boundary(j)?;
        Some((j, String::from_utf8_lossy(&self.s[i..j]).into_owned()))
    }

    /// `variant = (repeat(5, 8)[alnum] | (digit >> repeat(3, inf)[alnum])) >>
    /// (&lit('-') | eoi)`
    fn variant(&self, i: usize) -> Option<usize> {
        let j = self
            .repeat(i, 5, 8, alnum)
            .or_else(|| self.at(i).filter(|c| digit(*c)).and_then(|_| self.repeat(i + 1, 3, usize::MAX, alnum)))?;
        self.boundary(j)
    }

    /// `extension = singleton >> +("-" >> repeat(2, 8)[alnum]) >> (&lit('-') | eoi)`
    fn extension(&self, i: usize) -> Option<usize> {
        let mut j = self.singleton(i)?;
        j = self.lit(j, b'-').and_then(|k| self.repeat(k, 2, 8, alnum))?;
        while let Some(k) = self.lit(j, b'-').and_then(|k| self.repeat(k, 2, 8, alnum)) {
            j = k;
        }
        self.boundary(j)
    }

    /// `singleton = digit | char_('a', 'w') | char_('y', 'z')`.
    fn singleton(&self, i: usize) -> Option<usize> {
        let c = self.at(i)?;
        (digit(c) || (b'a'..=b'w').contains(&c) || (b'y'..=b'z').contains(&c)).then_some(i + 1)
    }

    /// `privateuse = "x" >> +("-" >> repeat(1, 8)[alnum]) >> (&lit('-') | eoi)`
    fn privateuse(&self, i: usize) -> Option<usize> {
        let mut j = self.lit_str(i, "x")?;
        j = self.lit(j, b'-').and_then(|k| self.repeat(k, 1, 8, alnum))?;
        while let Some(k) = self.lit(j, b'-').and_then(|k| self.repeat(k, 1, 8, alnum)) {
            j = k;
        }
        self.boundary(j)
    }

    /// `grandfathered = regular | irregular`
    fn grandfathered(&self, i: usize) -> Option<usize> {
        const REGULAR: [&str; 9] = [
            "art-lojban",
            "cel-gaulish",
            "no-bok",
            "no-nyn",
            "zh-guoyu",
            "zh-hakka",
            "zh-min",
            "zh-min-nan",
            "zh-xiang",
        ];
        const IRREGULAR: [&str; 17] = [
            "en-GB-oed",
            "i-ami",
            "i-bnn",
            "i-default",
            "i-enochian",
            "i-hak",
            "i-klingon",
            "i-lux",
            "i-mingo",
            "i-navajo",
            "i-pwn",
            "i-tao",
            "i-tay",
            "i-tsu",
            "sgn-BE-FR",
            "sgn-BE-NL",
            "sgn-CH-DE",
        ];
        REGULAR.iter().chain(IRREGULAR.iter()).find_map(|lit| self.lit_str(i, lit))
    }
}

impl LanguageTag {
    /// `LanguageTag(language, script, region)`.
    #[must_use]
    pub fn new(language: Option<String>, script: Option<String>, region: Option<String>) -> Self {
        LanguageTag { language, script, region }
    }

    /// `LanguageTag::fromBCP47`: the whole tag must parse.
    #[must_use]
    pub fn from_bcp47(bcp47_tag: &str) -> LanguageTag {
        let mut parser = Parser { s: bcp47_tag.as_bytes(), tag: LanguageTag::default() };
        match parser.start(0) {
            Some(end) if end == bcp47_tag.len() => parser.tag,
            // Invalid tags are treated as empty/"default".
            _ => LanguageTag::default(),
        }
    }

    /// `toBCP47`.
    #[must_use]
    pub fn to_bcp47(&self) -> String {
        let Some(language) = &self.language else {
            // BCP 47 requires a language, but we're matching implementations
            // that accept "" to mean something like "default".
            return String::new();
        };
        let mut bcp47 = language.clone();
        if let Some(script) = &self.script {
            bcp47.push('-');
            bcp47.push_str(script);
        }
        if let Some(region) = &self.region {
            bcp47.push('-');
            bcp47.push_str(region);
        }
        bcp47
    }
}

/// Ports `test/text/language_tag.test.cpp`.
#[cfg(test)]
mod tests {
    use super::*;

    // gtest: LanguageTag.Parsing
    #[test]
    fn parsing() {
        let bare_language = LanguageTag::from_bcp47("de");
        assert_eq!(bare_language.language.as_deref(), Some("de"));
        assert!(bare_language.script.is_none());
        assert!(bare_language.region.is_none());

        let language_and_script = LanguageTag::from_bcp47("de-hans");
        assert_eq!(language_and_script.language.as_deref(), Some("de"));
        assert_eq!(language_and_script.script.as_deref(), Some("hans"));
        assert!(language_and_script.region.is_none());

        let language_and_region = LanguageTag::from_bcp47("de-US");
        assert_eq!(language_and_region.language.as_deref(), Some("de"));
        assert!(language_and_region.script.is_none());
        assert_eq!(language_and_region.region.as_deref(), Some("US"));

        let kitchen_sink = LanguageTag::from_bcp47("de-ext-two-Hans-US-variant-x-private");
        assert_eq!(kitchen_sink.language.as_deref(), Some("de"));
        assert_eq!(kitchen_sink.script.as_deref(), Some("Hans"));
        assert_eq!(kitchen_sink.region.as_deref(), Some("US"));
    }

    // gtest: LanguageTag.Generation
    #[test]
    fn generation() {
        let de_hans_us = LanguageTag::new(Some("de".to_owned()), Some("hans".to_owned()), Some("US".to_owned()));
        assert_eq!(de_hans_us.to_bcp47(), "de-hans-US");

        let de_us = LanguageTag::new(Some("de".to_owned()), None, Some("US".to_owned()));
        assert_eq!(de_us.to_bcp47(), "de-US");

        let no_language = LanguageTag::new(None, Some("hans".to_owned()), Some("US".to_owned()));
        assert_eq!(no_language.to_bcp47(), "");

        let de_hans = LanguageTag::new(Some("de".to_owned()), Some("hans".to_owned()), None);
        assert_eq!(de_hans.to_bcp47(), "de-hans");

        let de = LanguageTag::new(Some("de".to_owned()), None, None);
        assert_eq!(de.to_bcp47(), "de");
    }
}
