//! Where a label breaks into lines.
//!
//! The Unicode line breaking algorithm (UAX #14) says where a line may
//! break. Of those places, the breaker picks the set whose lines come out
//! closest to an even width.

use mln_style::i18n;
use mln_vec_map::VecSet;
use unicode_linebreak::{BreakOpportunity, linebreaks};

use crate::atlas::ImagePositions;
use crate::glyph::GlyphPositions;
use crate::shaping::glyph_advances;
use crate::tagged_string::{SHAPED_GLYPH, TaggedString};

/// Zero width space, which suggests where a label may break, as in Japanese
/// labels.
const ZWSP: char = '\u{200B}';

/// What a glyph that a font face shaped stands for in line breaking: a
/// letter, so a line breaks around a shaped run as around a word, and never
/// inside it.
const SHAPED_GLYPH_STAND_IN: char = 'A';

/// A place where a line may start.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Opportunity {
    /// The index of the line's first character.
    index: usize,
    /// Whether the line must start there, after a newline or another
    /// line or paragraph separator.
    mandatory: bool,
}

/// Whether a line must break after `c`: UAX #14's classes BK, CR, LF and NL.
fn is_mandatory_break(c: char) -> bool {
    matches!(c, '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The text of `text` as line breaking reads it.
fn line_text(text: &TaggedString) -> String {
    (0..text.len())
        .map(|i| match text.text()[i] {
            SHAPED_GLYPH if text.section_of(i).shaped.is_some() => SHAPED_GLYPH_STAND_IN,
            c => c,
        })
        .collect()
}

/// The places in `text`, whose [`line_text`] is `line`, where a line may
/// start, in order, other than its start and end: where UAX #14 allows a
/// break, after an image, and before an opening parenthesis that follows a
/// word, as in "Freiburg(Breisgau)".
fn opportunities<'a>(text: &'a TaggedString, line: &'a str) -> impl Iterator<Item = Opportunity> + 'a {
    let len = text.len();
    let mut uax14 = linebreaks(line).peekable();
    let mut previous = None;
    line.char_indices().enumerate().filter_map(move |(index, (byte_offset, c))| {
        let before = previous.replace(c)?;
        let uax14_break = uax14.next_if(|&(offset, _)| offset == byte_offset);
        let breaks = uax14_break.is_some()
            || text.section_of(index - 1).image_id.is_some()
            || (c == '(' && index + 1 < len && before.is_alphanumeric());
        let mandatory = uax14_break.is_some_and(|(_, opportunity)| opportunity == BreakOpportunity::Mandatory);
        breaks.then_some(Opportunity { index, mandatory })
    })
}

/// How bad a break is besides the width of its line: very negative where a
/// line must break, and positive for a break that the text doesn't suggest
/// when it suggests others with zero width spaces.
fn penalty(opportunity: Opportunity, before: char, suggests_breaks: bool) -> f32 {
    if opportunity.mandatory {
        -10000.0
    } else if suggests_breaks && before != ZWSP && !i18n::is_whitespace(before) {
        150.0
    } else {
        0.0
    }
}

/// How far a line `line_width` wide is from `target_width`, plus the
/// `penalty` of the break that ends it. The last line may fall short of the
/// target more cheaply than it may exceed it.
fn badness(line_width: f32, target_width: f32, penalty: f32, is_last_break: bool) -> f32 {
    let raggedness = (line_width - target_width).powi(2);
    if is_last_break {
        return if line_width < target_width { raggedness / 2.0 } else { raggedness * 2.0 };
    }
    raggedness + penalty * penalty.abs()
}

/// A break, with the best breaks before it.
#[derive(Clone, Copy, Debug)]
struct Break {
    /// The index of the character that starts the next line.
    index: usize,
    /// How wide the text before the break is.
    x: f32,
    /// The best break before this one, an index into the breaks so far.
    prior: Option<usize>,
    /// The badness of the lines up to this break, with the best breaks.
    badness: f32,
}

/// The break at `index` and `x`, after the best of the `prior` breaks.
/// A line may exceed the maximum width: a label without a break wider than
/// it has to, and holding lines strictly to it gives lopsided lines where
/// the target width is close to it.
fn evaluate_break(index: usize, x: f32, target_width: f32, prior: &[Break], penalty: f32, is_last: bool) -> Break {
    let mut best = Break { index, x, prior: None, badness: badness(x, target_width, penalty, is_last) };
    for (k, prior_break) in prior.iter().enumerate() {
        let badness = badness(x - prior_break.x, target_width, penalty, is_last) + prior_break.badness;
        if badness <= best.badness {
            best.prior = Some(k);
            best.badness = badness;
        }
    }
    best
}

/// The indices of the characters that start lines when `text`, in logical
/// order, breaks into lines of about equal width, each at most `max_width`
/// wide where it can be. A `max_width` of 0 keeps the text on one line.
///
/// Breaking works in logical order because bidi reorders the text by line.
#[must_use]
pub(crate) fn determine_line_breaks(
    text: &TaggedString,
    spacing: f32,
    max_width: f32,
    glyphs: &GlyphPositions,
    image_positions: &ImagePositions,
    layout_text_size: f32,
) -> VecSet<usize> {
    if max_width == 0.0 || text.is_empty() {
        return VecSet::new();
    }

    let chars = text.text();
    let advances: Vec<f32> = glyph_advances(text, spacing, glyphs, image_positions, layout_text_size).collect();
    let advance = |i: usize| advances[i];
    let total_width: f32 = advances.iter().sum();
    // An infinite quotient saturates.
    let line_count = (total_width / max_width).ceil().max(1.0) as i32;
    // Text that fits on one line, without a line that must break, stays on
    // one: any break leaves a line shorter than the target, the whole text.
    if line_count == 1 && total_width > 0.0 && !chars.iter().copied().any(is_mandatory_break) {
        return VecSet::from([chars.len()]);
    }
    let target_width = total_width / line_count as f32;

    let suggests_breaks = chars.contains(&ZWSP);
    // A line's width leaves out its whitespace.
    let mut x = 0.0;
    let mut measured = 0;
    let mut measure_to = |end: usize| {
        for (i, &c) in chars.iter().enumerate().take(end).skip(measured) {
            if !i18n::is_whitespace(c) {
                x += advance(i);
            }
        }
        measured = end;
        x
    };

    let mut breaks: Vec<Break> = Vec::new();
    for opportunity in opportunities(text, &line_text(text)) {
        let x = measure_to(opportunity.index);
        let penalty = penalty(opportunity, chars[opportunity.index - 1], suggests_breaks);
        breaks.push(evaluate_break(opportunity.index, x, target_width, &breaks, penalty, false));
    }
    let last = evaluate_break(chars.len(), measure_to(chars.len()), target_width, &breaks, 0.0, true);

    let mut line_breaks = VecSet::from([last.index]);
    let mut prior = last.prior;
    while let Some(k) = prior {
        line_breaks.insert(breaks[k].index);
        prior = breaks[k].prior;
    }
    line_breaks
}

#[cfg(test)]
mod tests {
    use mln_style::types::VerticalAlign;

    use super::*;
    use crate::glyph::GlyphIDType;
    use crate::tagged_string::SectionOptions;

    fn indices(text: &str) -> Vec<usize> {
        let text = TaggedString::new(
            text.chars().collect(),
            SectionOptions::new(1.0, vec!["font".to_string()], GlyphIDType::FontPBF, None),
        );
        opportunities(&text, &line_text(&text)).map(|opportunity| opportunity.index).collect()
    }

    #[test]
    fn breaks_after_spaces_and_hyphens() {
        assert_eq!(indices("Rue de la Paix"), [4, 7, 10]);
        assert_eq!(indices("Saint-Denis"), [6]);
        assert_eq!(indices("A  B"), [3]);
    }

    #[test]
    fn breaks_between_ideographs() {
        assert_eq!(indices("東京駅"), [1, 2]);
        // Not before a closing bracket or after an opening one.
        assert_eq!(indices("東（京）駅"), [1, 4]);
    }

    #[test]
    fn keeps_parentheses_and_punctuation_with_their_words() {
        assert_eq!(indices("Paris (Centre)"), [6]);
        assert_eq!(indices("Freiburg(Breisgau) Hbf"), [8, 19]);
        // Not before a parenthesis that ends the text.
        assert_eq!(indices("Freiburg("), Vec::<usize>::new());
        assert_eq!(indices("St. Mary's, Ely"), [4, 12]);
    }

    #[test]
    fn breaks_after_zero_width_spaces_and_newlines() {
        // Not before one.
        assert_eq!(indices("中\u{200B}中\u{200B}"), [2]);
        let text =
            TaggedString::new("a\nb".chars().collect(), SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None));
        assert!(opportunities(&text, &line_text(&text)).eq([Opportunity { index: 2, mandatory: true }]));
    }

    #[test]
    fn breaks_after_images() {
        let mut text = TaggedString::default();
        text.add_image_section("icon", VerticalAlign::Bottom);
        text.add_text_section(&['a', 'b'], SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None));
        assert!(opportunities(&text, &line_text(&text)).eq([Opportunity { index: 1, mandatory: false }]));
    }
}
