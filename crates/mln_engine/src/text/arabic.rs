//! Arabic letter shaping for text drawn with glyph PBFs, whose glyphs come
//! without the OpenType tables that shape text: each letter takes the
//! presentation form of its place in a word instead. Text that HarfRust
//! shapes doesn't need it.
//!
//! The presentation forms come from the Unicode Character Database
//! (`arabic_tables`), in the Unicode version of unicode-bidi's tables. A
//! letter joins on the sides its forms connect, so that a letter without
//! forms, which a glyph PBF can only draw on its own, isn't joined. The
//! tables hold BMP characters as `u16`.

use std::borrow::Cow;

use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::text::arabic_tables::{LAM_ALEF_FORMS, LETTER_FORMS, MARK_FORMS};

const LAM: char = '\u{0644}';

/// `c` as a key of the tables, if it is in the BMP.
fn key(c: char) -> Option<u16> {
    u16::try_from(u32::from(c)).ok()
}

/// A character from the tables, which hold no surrogates.
fn from_table(unit: u16) -> char {
    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// How a character joins the characters around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Joining {
    /// Joins the character before it in logical order if `prev`, and the
    /// one after it if `next`, when that one joins it.
    Joins { prev: bool, next: bool },
    /// Doesn't break a join between the characters around it, as marks
    /// don't.
    Transparent,
}

impl Joining {
    const NONE: Self = Joining::Joins { prev: false, next: false };

    /// How a character joins.
    fn of(c: char) -> Self {
        if let Some(Ok(i)) = key(c).map(|unit| LETTER_FORMS.binary_search_by_key(&unit, |&(letter, _)| letter)) {
            let [_, final_, initial, medial] = LETTER_FORMS[i].1;
            return Joining::Joins { prev: final_ != 0 || medial != 0, next: initial != 0 || medial != 0 };
        }
        match c {
            // Tatweel and zero width joiner.
            '\u{0640}' | '\u{200D}' => Joining::Joins { prev: true, next: true },
            // The format characters that ArabicShaping.txt makes
            // non-joining: the number signs, the Mongolian vowel separator,
            // the zero width non-joiner and the BiDi isolates.
            '\u{0600}'..='\u{0605}'
            | '\u{06DD}'
            | '\u{0890}'
            | '\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200C}'
            | '\u{2066}'..='\u{2069}'
            | '\u{110BD}'
            | '\u{110CD}' => Joining::NONE,
            _ => match c.general_category() {
                GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark | GeneralCategory::Format => {
                    Joining::Transparent
                }
                _ => Joining::NONE,
            },
        }
    }

    /// Whether the character joins the one after it, if that one joins it.
    fn joins_next(self) -> bool {
        matches!(self, Joining::Joins { next: true, .. })
    }

    /// Whether the character joins the one before it, if that one joins it.
    fn joins_prev(self) -> bool {
        matches!(self, Joining::Joins { prev: true, .. })
    }
}

/// Shapes text in logical order for glyph PBFs.
///
/// Each joining letter takes its isolated, final, initial or medial
/// presentation form by the letters it joins, a lam and the alef after it
/// become their ligature, and the marks with presentation forms (fathatan
/// to sukun) take their form on a tatweel between two joined letters, or
/// their form on their own otherwise. Transparent characters, such as
/// marks, don't break a join, and a letter without presentation forms joins
/// neither side. Presentation forms of letters in `text` are read as their
/// letters first. Returns `text` itself if it has no Arabic.
#[must_use]
pub fn shape_arabic(text: &[char]) -> Cow<'_, [char]> {
    if !text.iter().any(|&c| matches!(c, '\u{0600}'..='\u{06FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')) {
        return Cow::Borrowed(text);
    }
    let letters = to_letters(text);
    let joinings: Vec<Joining> = letters.iter().map(|&c| Joining::of(c)).collect();
    // The index and joining of the first character after `i` that isn't
    // transparent, or the end of the text, which doesn't join.
    let next = |i: usize| {
        (i + 1..letters.len())
            .find(|&j| joinings[j] != Joining::Transparent)
            .map_or((letters.len(), Joining::NONE), |j| (j, joinings[j]))
    };

    let mut shaped = Vec::with_capacity(letters.len());
    // The joining of the last character that isn't transparent.
    let mut prev = Joining::NONE;
    let mut i = 0;
    while i < letters.len() {
        let (c, joining) = (letters[i], joinings[i]);
        let (next_index, next_joining) = next(i);
        if joining == Joining::Transparent {
            shaped.push(mark_form(c, prev.joins_next() && next_joining.joins_prev()));
            i += 1;
            continue;
        }
        let joins_prev = joining.joins_prev() && prev.joins_next();
        if c == LAM
            && let Some(ligature) = letters.get(next_index).and_then(|&alef| lam_alef_forms(alef))
        {
            shaped.push(ligature[usize::from(joins_prev)]);
            // The ligature has no tatweel for the marks between them.
            shaped.extend(letters[i + 1..next_index].iter().map(|&mark| mark_form(mark, false)));
            prev = next_joining;
            i = next_index + 1;
            continue;
        }
        let joins_next = joining.joins_next() && next_joining.joins_prev();
        shaped.push(letter_form(c, joins_prev, joins_next));
        prev = joining;
        i += 1;
    }
    Cow::Owned(shaped)
}

/// `text` with each presentation form of a letter replaced by the letter,
/// and each lam-alef ligature by lam and alef.
fn to_letters(text: &[char]) -> Vec<char> {
    let mut letters = Vec::with_capacity(text.len());
    for &c in text {
        let form = key(c).filter(|form| matches!(form, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF));
        let Some(form) = form else {
            letters.push(c);
            continue;
        };
        if let Some(&(letter, _)) = LETTER_FORMS.iter().find(|(_, forms)| forms.contains(&form)) {
            letters.push(from_table(letter));
        } else if let Some(&(alef, _)) = LAM_ALEF_FORMS.iter().find(|(_, forms)| forms.contains(&form)) {
            letters.extend([LAM, from_table(alef)]);
        } else {
            letters.push(c);
        }
    }
    letters
}

/// The presentation form of a letter that joins the letters before and
/// after it as given, or the letter itself if it has no such form.
fn letter_form(c: char, joins_prev: bool, joins_next: bool) -> char {
    let Some(Ok(i)) = key(c).map(|unit| LETTER_FORMS.binary_search_by_key(&unit, |&(letter, _)| letter)) else {
        return c;
    };
    // Isolated, final, initial, medial.
    match LETTER_FORMS[i].1[usize::from(joins_prev) + 2 * usize::from(joins_next)] {
        0 => c,
        form => from_table(form),
    }
}

/// The isolated and final forms of the ligature of lam and `alef`, if they
/// have one.
fn lam_alef_forms(alef: char) -> Option<[char; 2]> {
    let alef = key(alef)?;
    let i = LAM_ALEF_FORMS.binary_search_by_key(&alef, |&(alef, _)| alef).ok()?;
    Some(LAM_ALEF_FORMS[i].1.map(from_table))
}

/// The presentation form of a mark, on a tatweel if `on_tatweel` and it has
/// that form, or on its own; or the mark itself if it has no forms.
fn mark_form(c: char, on_tatweel: bool) -> char {
    let Some(Ok(i)) = key(c).map(|unit| MARK_FORMS.binary_search_by_key(&unit, |&(mark, _)| mark)) else {
        return c;
    };
    match MARK_FORMS[i].1 {
        [_, medial] if on_tatweel && medial != 0 => from_table(medial),
        [isolated, _] => from_table(isolated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn shape(s: &str) -> Vec<char> {
        shape_arabic(&u(s)).into_owned()
    }

    // These three mirror the unit tests in mapbox-gl-rtl-text's
    // arabic.test.js.
    #[test]
    fn arabic_shaping() {
        assert_eq!(shape("اليَمَن\u{200E}\u{200E}"), u("ﺍﻟﻴﹷﻤﹷﻦ\u{200E}\u{200E}"));
    }

    #[test]
    fn tashkeel() {
        assert_eq!(shape("سلام۳۹"), u("ﺳﻼﻡ۳۹"));
    }

    #[test]
    fn mixed_shaping() {
        assert_eq!(
            shape("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah"),
            u("ﻣﻜﺘﺒﺔ ﺍﻹﺳﻜﻨﺪﺭﻳﺔ\u{200E}\u{200E} Maktabat al-Iskandarīyah")
        );
    }

    #[test]
    fn marks_beyond_the_bmp_dont_break_a_join() {
        assert_eq!(shape("ب\u{E0100}ب"), ['\u{FE91}', '\u{E0100}', '\u{FE90}']);
    }

    #[test]
    fn hamza_takes_its_isolated_form() {
        assert_eq!(shape("ء"), ['\u{FE80}']);
        assert_eq!(shape("\u{FE80}"), ['\u{FE80}']);
    }

    #[test]
    fn lam_alef_with_a_mark_between_them() {
        assert_eq!(shape("لَا"), ['\u{FEFB}', '\u{FE76}']);
    }

    #[test]
    fn zero_width_non_joiner_breaks_a_join() {
        assert_eq!(shape("ب\u{200C}ب"), ['\u{FE8F}', '\u{200C}', '\u{FE8F}']);
    }

    #[test]
    fn letters_without_forms_dont_join() {
        assert_eq!(shape("ب\u{0750}ب"), ['\u{FE8F}', '\u{0750}', '\u{FE8F}']);
    }

    #[test]
    fn text_without_arabic_is_borrowed() {
        let text = u("Maktabat al-Iskandarīyah");
        assert!(matches!(shape_arabic(&text), Cow::Borrowed(t) if t == text));
    }
}
