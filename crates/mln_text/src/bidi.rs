//! Lines and runs of text in visual order.
//!
//! The `unicode-bidi` crate resolves levels and visual runs (UAX #9), and
//! this module writes them: runs in visual order, RTL runs reversed by
//! character with each character replaced by its `Bidi_Mirroring_Glyph`
//! (rule L4, which unicode-bidi leaves to the renderer; `crate::bidi_mirroring`,
//! from the same Unicode version as unicode-bidi's tables), and BiDi controls
//! removed.
//!
//! The line splitters take line break points as offsets into the text, in
//! code units. Each must end a nonempty line within the text (`0 < point <=
//! len`); the end of each paragraph is a line break point whether or not the
//! caller passes it.

use std::ops::Range;

use mln_vec_map::VecSet;
use unicode_bidi::BidiClass;
use unicode_bidi::utf16::BidiInfo;

use crate::bidi_mirroring::BIDI_MIRRORING_GLYPH;
use crate::tagged_string::StyledText;

/// Whether the first character in `text` with a strong direction runs right
/// to left. Unpaired surrogates have no direction.
#[must_use]
pub fn starts_right_to_left(text: &[u16]) -> bool {
    for ch in char::decode_utf16(text.iter().copied()).flatten() {
        match unicode_bidi::bidi_class(ch) {
            BidiClass::L => return false,
            BidiClass::R | BidiClass::AL => return true,
            _ => {}
        }
    }
    false
}

/// The character's `Bidi_Mirroring_Glyph`, or the character itself if it has
/// none.
fn mirror(c: char) -> char {
    u16::try_from(u32::from(c))
        .ok()
        .and_then(|c| BIDI_MIRRORING_GLYPH.binary_search_by_key(&c, |&(from, _)| from).ok())
        .and_then(|i| char::from_u32(u32::from(BIDI_MIRRORING_GLYPH[i].1)))
        .unwrap_or(c)
}

/// Whether `c` is a BiDi control that visual-order text leaves out.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        // ZWNJ, ZWJ, LRM and RLM
        '\u{200C}'..='\u{200F}'
        // LRE, RLE, PDF, LRO and RLO
        | '\u{202A}'..='\u{202E}'
        // LRI, RLI, FSI and PDI
        | '\u{2066}'..='\u{2069}'
    )
}

/// Appends the run `src` to `out` in visual order without BiDi controls: an
/// RTL run reversed by character, each character mirrored, and an LTR run as
/// it is. Unpaired surrogates are kept as characters of their own.
fn write_run(src: &[u16], rtl: bool, out: &mut Vec<u16>) {
    let start = out.len();
    for c in char::decode_utf16(src.iter().copied()) {
        match c {
            Ok(c) if is_bidi_control(c) => {}
            Ok(c) if rtl => {
                let mut units = [0; 2];
                let units = mirror(c).encode_utf16(&mut units);
                units.reverse();
                out.extend_from_slice(units);
            }
            Ok(c) => out.extend_from_slice(c.encode_utf16(&mut [0; 2])),
            Err(e) => out.push(e.unpaired_surrogate()),
        }
    }
    if rtl {
        // Each character went in with its code units reversed, so reversing
        // the whole run reverses the characters and puts their units back in
        // order.
        out[start..].reverse();
    }
}

/// A run of text in one direction, in logical order.
#[derive(Clone, Debug, PartialEq)]
pub struct BiDiRun {
    pub text: Vec<u16>,
    pub rtl: bool,
}

/// The lines of `info`'s text that end at `line_breaks` and at the ends of
/// its paragraphs, so that no line crosses a paragraph boundary.
fn lines(info: &BidiInfo<'_>, mut line_breaks: VecSet<usize>) -> impl Iterator<Item = Range<usize>> {
    let len = info.text.len();
    debug_assert!(
        line_breaks.iter().all(|&point| 0 < point && point <= len),
        "line breaks {line_breaks:?} outside 1..={len}"
    );
    for paragraph in &info.paragraphs {
        line_breaks.insert(paragraph.range.end);
    }
    let mut start = 0;
    // A release build drops points outside the text rather than panic.
    line_breaks.into_iter().filter(move |&end| 0 < end && end <= len).map(move |end| {
        let line = start..end;
        start = end;
        line
    })
}

/// The visual runs of `line`, a nonempty line of `info`'s text within one
/// paragraph: whether each runs right to left, and its logical range, in
/// visual order.
fn line_runs(info: &BidiInfo<'_>, line: Range<usize>) -> Vec<(bool, Range<usize>)> {
    let paragraph = &info.paragraphs[info.paragraphs.partition_point(|p| p.range.end <= line.start)];
    let (levels, runs) = info.visual_runs(paragraph, line);
    runs.into_iter().map(|run| (levels[run.start].is_rtl(), run)).collect()
}

/// Splits `text`, in logical order, into lines at `line_breaks`, and writes
/// each line in visual order.
pub(crate) fn reorder_lines(text: &[u16], line_breaks: VecSet<usize>) -> Vec<Vec<u16>> {
    let info = BidiInfo::new(text, None);
    lines(&info, line_breaks)
        .map(|line| {
            let mut out = Vec::with_capacity(line.len());
            for (rtl, run) in line_runs(&info, line) {
                write_run(&text[run], rtl, &mut out);
            }
            out
        })
        .collect()
}

/// Like [`reorder_lines`], with each code unit's style, except that LTR runs
/// keep their BiDi controls, and an RTL run's first code unit takes the
/// style of the chunk after it unless either style is a block.
pub(crate) fn reorder_styled_lines(input: &StyledText, line_breaks: VecSet<usize>) -> Vec<StyledText> {
    reorder_styled_lines_with_blocks(input, line_breaks, &input.text, &[])
}

/// Splits text in logical order into lines at `line_breaks` and at the ends
/// of paragraphs, without reordering it.
pub(crate) fn split_styled_lines(input: &StyledText, line_breaks: VecSet<usize>) -> Vec<StyledText> {
    let info = BidiInfo::new(&input.text, None);
    lines(&info, line_breaks)
        .map(|line| StyledText { text: input.text[line.clone()].to_vec(), styles: input.styles[line].to_vec() })
        .collect()
}

/// Like [`reorder_styled_lines`], but the bidi analysis reads `bidi_text`,
/// which is as long as the input's text, and a right-to-left run keeps the
/// order of the text in the styles that `block_styles` marks, which a shaper
/// already laid out in visual order.
pub(crate) fn reorder_styled_lines_with_blocks(
    input: &StyledText,
    line_breaks: VecSet<usize>,
    bidi_text: &[u16],
    block_styles: &[bool],
) -> Vec<StyledText> {
    debug_assert_eq!(bidi_text.len(), input.text.len());
    let input_text = &input.text;
    let style_indices = &input.styles;
    let is_block = |style: u8| block_styles.get(style as usize).copied().unwrap_or(false);

    let info = BidiInfo::new(bidi_text, None);
    let mut out = Vec::new();
    for line_range in lines(&info, line_breaks) {
        let mut line = StyledText { text: Vec::new(), styles: Vec::with_capacity(line_range.len()) };

        for (is_reversed, run) in line_runs(&info, line_range) {
            let logical_start = run.start;
            let logical_end = run.end;
            if is_reversed {
                // Within this reversed section, iterate logically
                // backwards. Each time we see a change in style, render
                // a reversed chunk of everything since the last change.
                //
                let mut style_run_start = logical_end;
                let mut current_style_index = style_indices[style_run_start - 1];
                let mut add_chunk = |start: usize, end: usize, style: u8| {
                    // A block's text is already in visual order.
                    let mut reversed = Vec::new();
                    if is_block(style) {
                        reversed.extend_from_slice(&input_text[start..end]);
                    } else {
                        write_run(&input_text[start..end], true, &mut reversed);
                    }
                    line.text.extend_from_slice(&reversed);
                    line.styles.extend(std::iter::repeat_n(style, reversed.len()));
                };
                let mut i = logical_end - 1;
                while i >= logical_start {
                    if current_style_index != style_indices[i] || i == logical_start {
                        // The run's first character joins the chunk
                        // after it even when its style differs, as GL JS
                        // lays it out, except next to a block, which
                        // holds only its own text.
                        let split_first = i == logical_start
                            && current_style_index != style_indices[i]
                            && (is_block(current_style_index) || is_block(style_indices[i]));
                        let style_run_end = if i == logical_start && !split_first { i } else { i + 1 };
                        add_chunk(style_run_end, style_run_start, current_style_index);
                        current_style_index = style_indices[i];
                        style_run_start = style_run_end;
                        if split_first {
                            add_chunk(i, i + 1, current_style_index);
                            style_run_start = i;
                        }
                    }
                    if i == 0 {
                        break;
                    }
                    i -= 1;
                }
            } else {
                line.text.extend_from_slice(&input_text[logical_start..logical_end]);
                line.styles.extend_from_slice(&style_indices[logical_start..logical_end]);
            }
        }

        out.push(line);
    }
    out
}

/// Splits text in logical order into runs of one direction each, and
/// returns the runs in visual order: each paragraph's runs in visual order,
/// the paragraphs in logical order. Each run keeps its logical order, so a
/// shaper such as HarfBuzz can lay it out in its direction. Text with no runs
/// is one LTR run.
#[must_use]
pub fn visual_runs(text: &[u16]) -> Vec<BiDiRun> {
    let mut runs = Vec::new();
    if text.is_empty() {
        return runs;
    }

    let info = BidiInfo::new(text, None);
    for paragraph in &info.paragraphs {
        for (rtl, run) in line_runs(&info, paragraph.range.clone()) {
            if run.is_empty() {
                continue;
            }
            runs.push(BiDiRun { text: text[run].to_vec(), rtl });
        }
    }

    if runs.is_empty() {
        runs.push(BiDiRun { text: text.to_vec(), rtl: false });
    }
    runs
}

/// These tests mirror the unit tests in mapbox-gl-rtl-text's arabic.test.js.
#[cfg(test)]
mod bidi_test {
    use super::*;
    use crate::arabic::shape_arabic;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn set(v: &[usize]) -> VecSet<usize> {
        v.iter().copied().collect()
    }

    fn styled(text: Vec<u16>, styles: Vec<u8>) -> StyledText {
        StyledText { text, styles }
    }

    // gtest: BiDi.ReverseArabic
    #[test]
    fn reverse_arabic() {
        assert_eq!(reorder_lines(&shape_arabic(&u("سلام۳۹")), set(&[])), vec![u("۳۹ﻡﻼﺳ")]);
    }

    // gtest: BiDi.ReverseMixed
    #[test]
    fn reverse_mixed() {
        assert_eq!(
            reorder_lines(&shape_arabic(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")), set(&[])),
            vec![u(" Maktabat al-Iskandarīyahﺔﻳﺭﺪﻨﻜﺳﻹﺍ ﺔﺒﺘﻜﻣ")]
        );
    }

    // gtest: BiDi.WithLineBreaks
    #[test]
    fn with_line_breaks() {
        let expected = vec![u(" ﺔﻳﺭﺪﻨﻜﺳﻹﺍ ﺔﺒﺘﻜﻣ"), u("Maktabat al-"), u("Iskandarīyah")];
        assert_eq!(
            reorder_lines(
                &shape_arabic(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")),
                set(&[18, 30])
            ),
            expected
        );
    }

    // gtest: BiDi.StyledText
    #[test]
    fn styled_text() {
        // This test uses line breaks that intentionally split/reorder/
        // interleave styled blocks that are contiguous in the input
        let input = styled(
            shape_arabic(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")).into_owned(),
            vec![
                0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 4, 5, 5, 5, 5, 6, 6, 6, 6, 6,
                6, 6, 6, 6, 6, 7, 7, 7,
            ],
        );

        let expected: Vec<StyledText> = vec![
            styled(u("ﺔﺒﺘﻜﻣ"), vec![0, 0, 0, 0, 0]),
            styled(u(" \u{200E}\u{200E}ﺔﻳﺭﺪﻨﻜﺳﻹﺍ "), vec![2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1]),
            styled(u("Maktabat al-"), vec![2, 3, 3, 3, 3, 3, 4, 5, 5, 5, 5, 6]),
            styled(u("Iskandarīyah"), vec![6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7]),
        ];
        for line in &expected {
            assert_eq!(line.text.len(), line.styles.len());
        }

        assert_eq!(reorder_styled_lines(&input, set(&[5, 18, 30])), expected);
    }

    // gtest: BiDi.VisualRuns
    #[test]
    fn visual_runs_split_by_direction() {
        let runs = visual_runs(&u("Erbil هەولێر"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, u("Erbil "));
        assert!(!runs[0].rtl);
        assert_eq!(runs[1].text, u("هەولێر"));
        assert!(runs[1].rtl);
    }

    // gtest: BiDi.VisualRunsRightToLeftParagraph
    #[test]
    fn visual_runs_right_to_left_paragraph() {
        // The runs come in visual order, left to right, each in logical order.
        let runs = visual_runs(&u("هەولێر Erbil"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, u("Erbil"));
        assert!(!runs[0].rtl);
        assert_eq!(runs[1].text, u("هەولێر "));
        assert!(runs[1].rtl);
    }

    // gtest: BiDi.BlocksKeepTheirOrderInRightToLeftRuns
    #[test]
    fn blocks_keep_their_order_in_right_to_left_runs() {
        // "XY" stands for glyphs that a shaper already laid out right to
        // left. Bidi sees them as Hebrew letters, and moves them without
        // reversing them.
        let input = styled(u("XY Erbil"), vec![0, 0, 1, 1, 1, 1, 1, 1]);
        let lines = reorder_styled_lines_with_blocks(&input, set(&[]), &u("אא Erbil"), &[true, false]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, u("Erbil XY"));
        assert_eq!(lines[0].styles, vec![1, 1, 1, 1, 1, 1, 0, 0]);
    }

    // gtest: BiDi.SplitStyledTextKeepsLogicalOrder
    #[test]
    fn split_styled_text_keeps_logical_order() {
        let lines = split_styled_lines(&styled(u("سلام\nabc"), vec![0, 0, 0, 0, 0, 1, 1, 1]), set(&[]));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, u("سلام\n"));
        assert_eq!(lines[1].text, u("abc"));
    }

    // gtest: BiDi.BlockAtTheStartOfARunTakesOnlyItsOwnText
    #[test]
    fn block_at_the_start_of_a_run_takes_only_its_own_text() {
        // The run "-XY" reads right to left, and its first character, "-",
        // is not part of the block.
        let input = styled(u("-XY"), vec![0, 1, 1]);
        let lines = reorder_styled_lines_with_blocks(&input, set(&[]), &u("-אא"), &[false, true]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, u("XY-"));
        assert_eq!(lines[0].styles, vec![1, 1, 0]);
    }

    #[test]
    fn starts_right_to_left_at_the_first_strong_character() {
        assert!(!starts_right_to_left(&u("1 abc שלום")));
        assert!(starts_right_to_left(&u("1 שלום abc")));
        assert!(starts_right_to_left(&u("(سلام)")));
        // A supplementary letter (Adlam), as a surrogate pair.
        assert!(starts_right_to_left(&u("\u{1E900}")));
        assert!(!starts_right_to_left(&u("123 !?")));
    }

    #[test]
    fn mirrors_the_characters_unicode_mirrors() {
        assert_eq!(mirror('('), ')');
        assert_eq!(mirror('<'), '>');
        assert_eq!(mirror('«'), '»');
        assert_eq!(mirror('≤'), '≥');
        assert_eq!(mirror('a'), 'a');
        assert_eq!(mirror('\u{1E900}'), '\u{1E900}');
    }

    #[test]
    fn writes_rtl_runs_mirrored() {
        let mut out = Vec::new();
        write_run(&u("א<ב(ג"), true, &mut out);
        assert_eq!(out, u("ג)ב>א"));
    }

    #[test]
    fn writes_runs_without_bidi_controls() {
        let mut out = Vec::new();
        write_run(&u("a\u{200D}b\u{202A}c\u{2069}(\u{200F}"), false, &mut out);
        assert_eq!(out, u("abc("));
    }

    #[test]
    fn reverses_rtl_runs_by_character() {
        // A surrogate pair stays in order; unpaired surrogates move alone.
        let mut out = Vec::new();
        let mut src = u("א\u{1E900}ב");
        src.extend([0xDC00, 0xD800]);
        write_run(&src, true, &mut out);
        let mut expected = vec![0xD800, 0xDC00];
        expected.extend(u("ב\u{1E900}א"));
        assert_eq!(out, expected);
    }

    #[test]
    fn mirroring_table_is_sorted_and_symmetric() {
        assert!(BIDI_MIRRORING_GLYPH.windows(2).all(|w| w[0].0 < w[1].0));
        for &(from, to) in &BIDI_MIRRORING_GLYPH {
            let (from, to) = (char::from_u32(from.into()).unwrap(), char::from_u32(to.into()).unwrap());
            assert_eq!(mirror(to), from, "U+{:04X}", u32::from(from));
        }
    }
}
