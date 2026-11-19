//! Arabic letter shaping for text drawn with glyph PBFs, whose glyphs come
//! without the OpenType tables that shape text: each letter takes the
//! presentation form of its place in a word instead. Text that HarfRust
//! shapes doesn't need it.
//!
//! Joining types and presentation forms come from the Unicode Character
//! Database (`arabic_tables`), in the Unicode version of unicode-bidi's
//! tables.

use std::borrow::Cow;

use crate::arabic_tables::{JOINING_TYPES, LAM_ALEF_FORMS, LETTER_FORMS, MARK_FORMS};

const LAM: u16 = 0x0644;

/// A character's Unicode `Joining_Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JoiningType {
    NonJoining,
    Dual,
    Right,
    Left,
    JoinCausing,
    Transparent,
}

impl JoiningType {
    /// The joining type of a UTF-16 code unit. Surrogates, like the
    /// characters outside the BMP that they encode, are non-joining here.
    fn of(unit: u16) -> Self {
        let i = JOINING_TYPES.partition_point(|&(_, last, _)| last < unit);
        match JOINING_TYPES.get(i) {
            Some(&(first, _, joining_type)) if first <= unit => joining_type,
            _ => JoiningType::NonJoining,
        }
    }

    /// Whether the character joins the one before it in logical order, if
    /// that one joins it.
    fn joins_preceding(self) -> bool {
        matches!(self, JoiningType::Dual | JoiningType::Right | JoiningType::JoinCausing)
    }

    /// Whether the character joins the one after it in logical order, if
    /// that one joins it.
    fn joins_following(self) -> bool {
        matches!(self, JoiningType::Dual | JoiningType::Left | JoiningType::JoinCausing)
    }
}

/// Shapes UTF-16 text in logical order for glyph PBFs.
///
/// Each joining letter takes its isolated, final, initial or medial
/// presentation form by the letters it joins, a lam and the alef after it
/// become their ligature, and the marks with presentation forms (fathatan
/// to sukun) take their form on a tatweel between two joined letters, or
/// their form on their own otherwise. Transparent characters, such as
/// marks, don't break a join. Presentation forms of letters in `text` are
/// read as their letters first. Returns `text` itself if it has no Arabic.
#[must_use]
pub fn shape_arabic(text: &[u16]) -> Cow<'_, [u16]> {
    if !text.iter().any(|&unit| matches!(unit, 0x0600..=0x06FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)) {
        return Cow::Borrowed(text);
    }
    let letters = to_letters(text);
    let types: Vec<JoiningType> = letters.iter().map(|&unit| JoiningType::of(unit)).collect();
    // The index and joining type of the first character after `i` that
    // isn't transparent, or the end of the text, which doesn't join.
    let next = |i: usize| {
        (i + 1..letters.len())
            .find(|&j| types[j] != JoiningType::Transparent)
            .map_or((letters.len(), JoiningType::NonJoining), |j| (j, types[j]))
    };

    let mut shaped = Vec::with_capacity(letters.len());
    // The joining type of the last character that isn't transparent.
    let mut prev = JoiningType::NonJoining;
    let mut i = 0;
    while i < letters.len() {
        let (unit, joining_type) = (letters[i], types[i]);
        let (next_index, next_type) = next(i);
        if joining_type == JoiningType::Transparent {
            shaped.push(mark_form(unit, prev.joins_following() && next_type.joins_preceding()));
            i += 1;
            continue;
        }
        let joins_prev = joining_type.joins_preceding() && prev.joins_following();
        if unit == LAM
            && let Some(ligature) = letters.get(next_index).and_then(|&alef| lam_alef_forms(alef))
        {
            shaped.push(ligature[usize::from(joins_prev)]);
            // The ligature has no tatweel for the marks between them.
            shaped.extend(letters[i + 1..next_index].iter().map(|&mark| mark_form(mark, false)));
            prev = next_type;
            i = next_index + 1;
            continue;
        }
        let joins_next = joining_type.joins_following() && next_type.joins_preceding();
        shaped.push(letter_form(unit, joins_prev, joins_next));
        prev = joining_type;
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
    fn text_without_arabic_is_borrowed() {
        let text = u("Maktabat al-Iskandarīyah");
        assert!(matches!(shape_arabic(&text), Cow::Borrowed(t) if t == text));
    }
}
