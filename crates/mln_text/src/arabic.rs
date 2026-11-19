//! Arabic letter shaping for text drawn with glyph PBFs, whose glyphs come
//! without the OpenType tables that shape text: each letter takes the
//! presentation form of its place in a word instead. Text that HarfRust
//! shapes doesn't need it.
//!
//! The presentation forms come from the Unicode Character Database
//! (`arabic_tables`), in the Unicode version of unicode-bidi's tables. A
//! letter joins on the sides its forms connect, so that a letter without
//! forms, which a glyph PBF can only draw on its own, isn't joined.

use std::borrow::Cow;

use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::arabic_tables::{LAM_ALEF_FORMS, LETTER_FORMS, MARK_FORMS};

const LAM: u16 = 0x0644;

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

    /// How a UTF-16 code unit joins. Surrogates don't.
    fn of(unit: u16) -> Self {
        if let Ok(i) = LETTER_FORMS.binary_search_by_key(&unit, |&(letter, _)| letter) {
            let [_, final_, initial, medial] = LETTER_FORMS[i].1;
            return Joining::Joins { prev: final_ != 0 || medial != 0, next: initial != 0 || medial != 0 };
        }
        match unit {
            // Tatweel and zero width joiner.
            0x0640 | 0x200D => Joining::Joins { prev: true, next: true },
            // The format characters that ArabicShaping.txt makes
            // non-joining: the number signs, the Mongolian vowel separator,
            // the zero width non-joiner and the BiDi isolates.
            0x0600..=0x0605 | 0x06DD | 0x0890 | 0x0891 | 0x08E2 | 0x180E | 0x200C | 0x2066..=0x2069 => Joining::NONE,
            _ => match char::from_u32(u32::from(unit)).map(UnicodeGeneralCategory::general_category) {
                Some(GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark | GeneralCategory::Format) => {
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

/// Shapes UTF-16 text in logical order for glyph PBFs.
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
pub fn shape_arabic(text: &[u16]) -> Cow<'_, [u16]> {
    if !text.iter().any(|&unit| matches!(unit, 0x0600..=0x06FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)) {
        return Cow::Borrowed(text);
    }
    let letters = to_letters(text);
    let joinings: Vec<Joining> = letters.iter().map(|&unit| Joining::of(unit)).collect();
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
        let (unit, joining) = (letters[i], joinings[i]);
        let (next_index, next_joining) = next(i);
        if joining == Joining::Transparent {
            shaped.push(mark_form(unit, prev.joins_next() && next_joining.joins_prev()));
            i += 1;
            continue;
        }
        let joins_prev = joining.joins_prev() && prev.joins_next();
        if unit == LAM
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
        shaped.push(letter_form(unit, joins_prev, joins_next));
        prev = joining;
        i += 1;
    }
    Cow::Owned(shaped)
}

/// `text` with each presentation form of a letter replaced by the letter,
/// and each lam-alef ligature by lam and alef.
fn to_letters(text: &[u16]) -> Vec<u16> {
    let mut letters = Vec::with_capacity(text.len());
    for &unit in text {
        if !matches!(unit, 0xFB50..=0xFDFF | 0xFE70..=0xFEFF) {
            letters.push(unit);
        } else if let Some(&(letter, _)) = LETTER_FORMS.iter().find(|(_, forms)| forms.contains(&unit)) {
            letters.push(letter);
        } else if let Some(&(alef, _)) = LAM_ALEF_FORMS.iter().find(|(_, forms)| forms.contains(&unit)) {
            letters.extend([LAM, alef]);
        } else {
            letters.push(unit);
        }
    }
    letters
}

/// The presentation form of a letter that joins the letters before and
/// after it as given, or the letter itself if it has no such form.
fn letter_form(unit: u16, joins_prev: bool, joins_next: bool) -> u16 {
    let Ok(i) = LETTER_FORMS.binary_search_by_key(&unit, |&(letter, _)| letter) else {
        return unit;
    };
    // Isolated, final, initial, medial.
    match LETTER_FORMS[i].1[usize::from(joins_prev) + 2 * usize::from(joins_next)] {
        0 => unit,
        form => form,
    }
}

/// The isolated and final forms of the ligature of lam and `alef`, if they
/// have one.
fn lam_alef_forms(alef: u16) -> Option<[u16; 2]> {
    let i = LAM_ALEF_FORMS.binary_search_by_key(&alef, |&(alef, _)| alef).ok()?;
    Some(LAM_ALEF_FORMS[i].1)
}

/// The presentation form of a mark, on a tatweel if `on_tatweel` and it has
/// that form, or on its own; or the mark itself if it has no forms.
fn mark_form(unit: u16, on_tatweel: bool) -> u16 {
    let Ok(i) = MARK_FORMS.binary_search_by_key(&unit, |&(mark, _)| mark) else {
        return unit;
    };
    match MARK_FORMS[i].1 {
        [_, medial] if on_tatweel && medial != 0 => medial,
        [isolated, _] => isolated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn shape(s: &str) -> Vec<u16> {
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
    fn hamza_takes_its_isolated_form() {
        assert_eq!(shape("ء"), [0xFE80]);
        assert_eq!(shape("\u{FE80}"), [0xFE80]);
    }

    #[test]
    fn lam_alef_with_a_mark_between_them() {
        assert_eq!(shape("لَا"), [0xFEFB, 0xFE76]);
    }

    #[test]
    fn zero_width_non_joiner_breaks_a_join() {
        assert_eq!(shape("ب\u{200C}ب"), [0xFE8F, 0x200C, 0xFE8F]);
    }

    #[test]
    fn letters_without_forms_dont_join() {
        assert_eq!(shape("ب\u{0750}ب"), [0xFE8F, 0x0750, 0xFE8F]);
    }

    #[test]
    fn text_without_arabic_is_borrowed() {
        let text = u("Maktabat al-Iskandarīyah");
        assert!(matches!(shape_arabic(&text), Cow::Borrowed(t) if t == text));
    }
}
