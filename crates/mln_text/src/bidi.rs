// The `unicode-bidi` crate resolves levels and visual runs (UAX #9), and
// this module writes them: runs in visual order, RTL runs reversed by code
// point with each character replaced by its Bidi_Mirroring_Glyph (rule L4,
// which unicode-bidi leaves to the renderer; crate::bidi_mirroring, from the
// same Unicode version as unicode-bidi's tables), and BiDi controls (ZWNJ,
// ZWJ, LRM, RLM, LRE..RLO, LRI..PDI) removed. Arabic letter shaping
// (`applyArabicShaping`) is a port of ICU's ushape.cpp (crate::icu::ushape).
//
// Where the C++ throws std::runtime_error, these return Err with the same
// message (`what()`).

use std::ops::Range;

use mln_vec_map::VecSet;
use unicode_bidi::utf16::BidiInfo;
use unicode_bidi::{BidiClass, ParagraphInfo};

use crate::bidi_mirroring::BIDI_MIRRORING_GLYPH;
use crate::icu::ushape::u_shape_arabic;
pub(crate) use crate::tagged_string::StyledText;

/// Takes UTF16 input in logical order and applies Arabic shaping to the
/// input while maintaining logical order. Output won't be intelligible
/// until the bidirectional algorithm is applied.
#[must_use]
pub fn apply_arabic_shaping(input: &[u16]) -> Vec<u16> {
    // (U_SHAPE_LETTERS_SHAPE & U_SHAPE_LETTERS_MASK) |
    // (U_SHAPE_TEXT_DIRECTION_LOGICAL & U_SHAPE_TEXT_DIRECTION_MASK)
    let (_, output_length, _) = u_shape_arabic(input, 0);

    // Pre-flighting will always set U_BUFFER_OVERFLOW_ERROR
    let mut output_text = vec![0u16; output_length.max(0) as usize];

    let (written, _, error_code) = u_shape_arabic(input, output_length);
    output_text[..written.len()].copy_from_slice(&written);

    // If the algorithm fails for any reason, fall back to non-transformed text
    if error_code.is_failure() {
        return input.to_vec();
    }

    output_text
}

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
fn mirror(c: u32) -> u32 {
    u16::try_from(c)
        .ok()
        .and_then(|c| BIDI_MIRRORING_GLYPH.binary_search_by_key(&c, |&(from, _)| from).ok())
        .map_or(c, |i| u32::from(BIDI_MIRRORING_GLYPH[i].1))
}

/// `IS_BIDI_CONTROL_CHAR(c)` (ICU ubidiimp.h): what
/// `UBIDI_REMOVE_BIDI_CONTROLS` removes.
fn is_bidi_control_char(c: u16) -> bool {
    let c = u32::from(c);
    (c & 0xffff_fffc) == 0x200c || c.wrapping_sub(0x202a) < 5 || c.wrapping_sub(0x2066) < 4
}

/// The code point ending at `text[end - 1]` and its first index.
fn code_point_before(text: &[u16], end: usize) -> (u32, usize) {
    let c = u32::from(text[end - 1]);
    if (c & 0xfc00) == 0xdc00 && end >= 2 {
        let lead = u32::from(text[end - 2]);
        if (lead & 0xfc00) == 0xd800 {
            return (((lead - 0xd800) << 10) + (c - 0xdc00) + 0x10000, end - 2);
        }
    }
    (c, end - 1)
}

fn push_code_point(out: &mut Vec<u16>, c: u32) {
    if c <= 0xffff {
        out.push(c as u16);
    } else {
        out.push(((c >> 10) + 0xd7c0) as u16);
        out.push(((c & 0x3ff) | 0xdc00) as u16);
    }
}

/// An RTL run written as `ubidi_writeReverse`/`writeReordered` do with
/// `UBIDI_DO_MIRRORING` | `UBIDI_REMOVE_BIDI_CONTROLS`: code points in reverse
/// order, mirrored, BiDi controls dropped.
fn write_reverse_mirrored(src: &[u16], out: &mut Vec<u16>) {
    let mut end = src.len();
    while end > 0 {
        let (c, start) = code_point_before(src, end);
        end = start;
        if c <= 0xffff && is_bidi_control_char(c as u16) {
            continue;
        }
        push_code_point(out, mirror(c));
    }
}

/// A run of text in one direction, in logical order.
#[derive(Clone, Debug, PartialEq)]
pub struct BiDiRun {
    pub text: Vec<u16>,
    pub rtl: bool,
}

#[derive(Default)]
pub struct BiDi {}

/// The paragraph of `info` that holds the line `[start, end)`, or the
/// error ICU's `ubidi_setLine` reports (an empty line, a line past the end
/// or across a paragraph boundary).
fn line_paragraph<'a>(info: &'a BidiInfo<'_>, start: usize, end: usize) -> Option<&'a ParagraphInfo> {
    if start >= end || end > info.text.len() {
        return None;
    }
    info.paragraphs.iter().find(|p| p.range.start <= start && end <= p.range.end)
}

/// Resolves the visual runs of a line: (is RTL, logical range), in visual
/// order (ICU's `ubidi_countRuns` / `ubidi_getVisualRun`).
fn visual_runs(info: &BidiInfo<'_>, para: &ParagraphInfo, line: Range<usize>) -> Vec<(bool, Range<usize>)> {
    let (levels, runs) = info.visual_runs(para, line);
    runs.into_iter().map(|run| (levels[run.start].is_rtl(), run)).collect()
}

impl BiDi {
    #[must_use]
    pub fn new() -> Self {
        BiDi {}
    }

    fn merge_paragraph_line_breaks(info: &BidiInfo<'_>, line_break_points: &mut VecSet<usize>) {
        for paragraph in &info.paragraphs {
            line_break_points.insert(paragraph.range.end);
        }
    }

    /// Given text in logical ordering and a set of line break points,
    /// return a set of lines in visual order with bidi and line breaking
    /// applied.
    #[expect(clippy::unused_self, reason = "the shaper threads one `BiDi` through, as the C++ shaping API does")]
    pub(crate) fn process_text(
        &mut self,
        input: &[u16],
        mut line_break_points: VecSet<usize>,
    ) -> Result<Vec<Vec<u16>>, String> {
        let info = BidiInfo::new(input, None);

        // BiDi::getLine will error if called across a paragraph boundary,
        // so we need to ensure that all paragraph boundaries are included
        // in the set of line break points. The calling code might not
        // include the line break because it didn't need to wrap at that
        // point, or because the text was separated with a more exotic code
        // point such as (U+001C)
        Self::merge_paragraph_line_breaks(&info, &mut line_break_points);

        let mut transformed_lines = Vec::with_capacity(line_break_points.len());

        let mut start = 0;
        for line_break_point in line_break_points {
            transformed_lines.push(Self::get_line(&info, start, line_break_point)?);
            start = line_break_point;
        }

        Ok(transformed_lines)
    }

    /// Same as processText but preserves per-code-point formatting
    /// information.
    pub(crate) fn process_styled_text(
        &mut self,
        input: &StyledText,
        line_break_points: VecSet<usize>,
    ) -> Result<Vec<StyledText>, String> {
        self.process_styled_text_with_blocks(input, line_break_points, &input.0, &[])
    }

    /// Splits text in logical order into lines at the line break points and
    /// at the ends of paragraphs, without reordering it.
    #[expect(clippy::unused_self, reason = "the shaper threads one `BiDi` through, as the C++ shaping API does")]
    #[expect(clippy::unnecessary_wraps, reason = "fails as the other line splitters do, which its callers treat alike")]
    pub(crate) fn split_styled_text(
        &mut self,
        input: &StyledText,
        mut line_break_points: VecSet<usize>,
    ) -> Result<Vec<StyledText>, String> {
        let info = BidiInfo::new(&input.0, None);
        Self::merge_paragraph_line_breaks(&info, &mut line_break_points);

        let mut lines = Vec::new();
        let mut line_start_index = 0usize;
        for line_break_point in line_break_points {
            lines.push((
                input.0[line_start_index..line_break_point].to_vec(),
                input.1[line_start_index..line_break_point].to_vec(),
            ));
            line_start_index = line_break_point;
        }
        Ok(lines)
    }

    /// The `processStyledText(input, lineBreakPoints, bidiText,
    /// blockStyles)` overload: same as processStyledText, but the bidi
    /// analysis reads `bidi_text`, which is as long as the input's text, and
    /// a right-to-left run keeps the order of the text in the styles that
    /// `block_styles` marks, which a shaper already laid out in visual
    /// order.
    #[expect(clippy::unused_self, reason = "the shaper threads one `BiDi` through, as the C++ shaping API does")]
    pub(crate) fn process_styled_text_with_blocks(
        &mut self,
        input: &StyledText,
        mut line_break_points: VecSet<usize>,
        bidi_text: &[u16],
        block_styles: &[bool],
    ) -> Result<Vec<StyledText>, String> {
        let mut lines = Vec::new();
        let input_text = &input.0;
        let style_indices = &input.1;
        let is_block = |style: u8| block_styles.get(style as usize).copied().unwrap_or(false);

        let info = BidiInfo::new(bidi_text, None);

        Self::merge_paragraph_line_breaks(&info, &mut line_break_points);

        let mut line_start_index = 0usize;

        for line_break_point in line_break_points {
            let mut line: StyledText =
                (Vec::new(), Vec::with_capacity(line_break_point.saturating_sub(line_start_index)));

            let Some(para) = line_paragraph(&info, line_start_index, line_break_point) else {
                return Err("BiDi::processStyledText (setLine): U_ILLEGAL_ARGUMENT_ERROR".to_string());
            };

            for (is_reversed, run) in visual_runs(&info, para, line_start_index..line_break_point) {
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
                            write_reverse_mirrored(&input_text[start..end], &mut reversed);
                        }
                        line.0.extend_from_slice(&reversed);
                        line.1.extend(std::iter::repeat_n(style, reversed.len()));
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
                    line.0.extend_from_slice(&input_text[logical_start..logical_end]);
                    line.1.extend_from_slice(&style_indices[logical_start..logical_end]);
                }
            }

            lines.push(line);
            line_start_index = line_break_point;
        }

        Ok(lines)
    }

    /// Splits text in logical order into runs of one direction each, and
    /// returns the runs in visual order. Each run keeps its logical order, so
    /// a shaper such as HarfBuzz can lay it out in its direction.
    pub fn visual_runs(&mut self, input: &[u16]) -> Vec<BiDiRun> {
        let mut runs = Vec::new();
        if input.is_empty() {
            return runs;
        }

        // ICU's ubidi_getVisualRun without ubidi_setLine: each paragraph's
        // runs in visual order, the paragraphs in logical order.
        let info = BidiInfo::new(input, None);
        for para in &info.paragraphs {
            for (rtl, run) in visual_runs(&info, para, para.range.clone()) {
                if run.is_empty() {
                    continue;
                }
                runs.push(BiDiRun { text: input[run].to_vec(), rtl });
            }
        }

        if runs.is_empty() {
            runs.push(BiDiRun { text: input.to_vec(), rtl: false });
        }
        runs
    }

    /// The line `start..end` in visual order, mirrored, with BiDi controls
    /// removed.
    fn get_line(info: &BidiInfo<'_>, start: usize, end: usize) -> Result<Vec<u16>, String> {
        let Some(para) = line_paragraph(info, start, end) else {
            return Err("BiDi::getLine (setLine): U_ILLEGAL_ARGUMENT_ERROR".to_string());
        };

        let mut output_text = Vec::with_capacity(end - start);
        for (is_reversed, run) in visual_runs(info, para, start..end) {
            let src = &info.text[run];
            if is_reversed {
                write_reverse_mirrored(src, &mut output_text);
            } else {
                // LTR runs are not mirrored.
                output_text.extend(src.iter().copied().filter(|&c| !is_bidi_control_char(c)));
            }
        }

        Ok(output_text)
    }
}

/// These tests mirror the unit tests in mapbox-gl-rtl-text's arabic.test.js.
#[cfg(test)]
mod bidi_test {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn set(v: &[usize]) -> VecSet<usize> {
        v.iter().copied().collect()
    }

    // gtest: BiDi.ArabicShaping
    #[test]
    fn arabic_shaping() {
        assert_eq!(apply_arabic_shaping(&u("اليَمَن\u{200E}\u{200E}")), u("ﺍﻟﻴﹷﻤﹷﻦ\u{200E}\u{200E}"));
    }

    // gtest: BiDi.Tashkeel
    #[test]
    fn tashkeel() {
        assert_eq!(apply_arabic_shaping(&u("سلام۳۹")), u("ﺳﻼﻡ۳۹"));
    }

    // gtest: BiDi.MixedShaping
    #[test]
    fn mixed_shaping() {
        assert_eq!(
            apply_arabic_shaping(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")),
            u("ﻣﻜﺘﺒﺔ ﺍﻹﺳﻜﻨﺪﺭﻳﺔ\u{200E}\u{200E} Maktabat al-Iskandarīyah")
        );
    }

    // gtest: BiDi.ReverseArabic
    #[test]
    fn reverse_arabic() {
        let mut bidi = BiDi::new();
        assert_eq!(bidi.process_text(&apply_arabic_shaping(&u("سلام۳۹")), set(&[])).unwrap(), vec![u("۳۹ﻡﻼﺳ")]);
    }

    // gtest: BiDi.ReverseMixed
    #[test]
    fn reverse_mixed() {
        let mut bidi = BiDi::new();
        assert_eq!(
            bidi.process_text(
                &apply_arabic_shaping(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")),
                set(&[])
            )
            .unwrap(),
            vec![u(" Maktabat al-Iskandarīyahﺔﻳﺭﺪﻨﻜﺳﻹﺍ ﺔﺒﺘﻜﻣ")]
        );
    }

    // gtest: BiDi.WithLineBreaks
    #[test]
    fn with_line_breaks() {
        let mut bidi = BiDi::new();
        let expected = vec![u(" ﺔﻳﺭﺪﻨﻜﺳﻹﺍ ﺔﺒﺘﻜﻣ"), u("Maktabat al-"), u("Iskandarīyah")];
        assert_eq!(
            bidi.process_text(
                &apply_arabic_shaping(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")),
                set(&[18, 30])
            )
            .unwrap(),
            expected
        );
    }

    // gtest: BiDi.StyledText
    #[test]
    fn styled_text() {
        // This test uses line breaks that intentionally split/reorder/
        // interleave styled blocks that are contiguous in the input
        let mut bidi = BiDi::new();
        let input: StyledText = (
            apply_arabic_shaping(&u("مكتبة الإسكندرية\u{200E}\u{200E} Maktabat al-Iskandarīyah")),
            vec![
                0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 4, 5, 5, 5, 5, 6, 6, 6, 6, 6,
                6, 6, 6, 6, 6, 7, 7, 7,
            ],
        );

        let expected: Vec<StyledText> = vec![
            (u("ﺔﺒﺘﻜﻣ"), vec![0, 0, 0, 0, 0]),
            (u(" \u{200E}\u{200E}ﺔﻳﺭﺪﻨﻜﺳﻹﺍ "), vec![2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1]),
            (u("Maktabat al-"), vec![2, 3, 3, 3, 3, 3, 4, 5, 5, 5, 5, 6]),
            (u("Iskandarīyah"), vec![6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7]),
        ];
        for line in &expected {
            assert_eq!(line.0.len(), line.1.len());
        }

        assert_eq!(bidi.process_styled_text(&input, set(&[5, 18, 30])).unwrap(), expected);
    }

    // gtest: BiDi.VisualRuns
    #[test]
    fn visual_runs() {
        let mut bidi = BiDi::new();
        let runs = bidi.visual_runs(&u("Erbil هەولێر"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, u("Erbil "));
        assert!(!runs[0].rtl);
        assert_eq!(runs[1].text, u("هەولێر"));
        assert!(runs[1].rtl);
    }

    // gtest: BiDi.VisualRunsRightToLeftParagraph
    #[test]
    fn visual_runs_right_to_left_paragraph() {
        let mut bidi = BiDi::new();
        // The runs come in visual order, left to right, each in logical order.
        let runs = bidi.visual_runs(&u("هەولێر Erbil"));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, u("Erbil"));
        assert!(!runs[0].rtl);
        assert_eq!(runs[1].text, u("هەولێر "));
        assert!(runs[1].rtl);
    }

    // gtest: BiDi.BlocksKeepTheirOrderInRightToLeftRuns
    #[test]
    fn blocks_keep_their_order_in_right_to_left_runs() {
        let mut bidi = BiDi::new();
        // "XY" stands for glyphs that a shaper already laid out right to
        // left. Bidi sees them as Hebrew letters, and moves them without
        // reversing them.
        let input: StyledText = (u("XY Erbil"), vec![0, 0, 1, 1, 1, 1, 1, 1]);
        let lines = bidi.process_styled_text_with_blocks(&input, set(&[]), &u("אא Erbil"), &[true, false]).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, u("Erbil XY"));
        assert_eq!(lines[0].1, vec![1, 1, 1, 1, 1, 1, 0, 0]);
    }

    // gtest: BiDi.SplitStyledTextKeepsLogicalOrder
    #[test]
    fn split_styled_text_keeps_logical_order() {
        let mut bidi = BiDi::new();
        let lines = bidi.split_styled_text(&(u("سلام\nabc"), vec![0, 0, 0, 0, 0, 1, 1, 1]), set(&[])).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, u("سلام\n"));
        assert_eq!(lines[1].0, u("abc"));
    }

    // gtest: BiDi.BlockAtTheStartOfARunTakesOnlyItsOwnText
    #[test]
    fn block_at_the_start_of_a_run_takes_only_its_own_text() {
        let mut bidi = BiDi::new();
        // The run "-XY" reads right to left, and its first character, "-",
        // is not part of the block.
        let input: StyledText = (u("-XY"), vec![0, 1, 1]);
        let lines = bidi.process_styled_text_with_blocks(&input, set(&[]), &u("-אא"), &[false, true]).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0, u("XY-"));
        assert_eq!(lines[0].1, vec![1, 1, 0]);
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
        assert_eq!(mirror('(' as u32), ')' as u32);
        assert_eq!(mirror('<' as u32), '>' as u32);
        assert_eq!(mirror('«' as u32), '»' as u32);
        assert_eq!(mirror('≤' as u32), '≥' as u32);
        assert_eq!(mirror('a' as u32), 'a' as u32);
        assert_eq!(mirror(0x1E900), 0x1E900);
    }

    #[test]
    fn writes_rtl_runs_mirrored() {
        let mut out = Vec::new();
        write_reverse_mirrored(&u("א<ב(ג"), &mut out);
        assert_eq!(out, u("ג)ב>א"));
    }

    #[test]
    fn mirroring_table_is_sorted_and_symmetric() {
        assert!(BIDI_MIRRORING_GLYPH.windows(2).all(|w| w[0].0 < w[1].0));
        for &(from, to) in &BIDI_MIRRORING_GLYPH {
            assert_eq!(mirror(u32::from(to)), u32::from(from), "U+{from:04X}");
        }
    }
}
