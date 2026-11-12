// Ports src/mln/text/bidi.hpp and platform/default/src/mln/text/bidi.cpp.
//
// The C++ runs ICU's ubidi (UBIDI_DEFAULT_LTR paragraphs, ubidi_setLine per
// line, ubidi_writeReordered / ubidi_writeReverse with UBIDI_DO_MIRRORING |
// UBIDI_REMOVE_BIDI_CONTROLS). The port resolves levels and visual runs
// with the `unicode-bidi` crate (UAX #9, Unicode 16) and does the writing
// itself: runs in visual order, RTL runs reversed by code point with each
// character replaced by its Bidi_Mirroring_Glyph (ICU's `u_charMirror`
// data, crate::icu::ubidi_props), BiDi controls (ZWNJ, ZWJ, LRM, RLM,
// LRE..RLO, LRI..PDI) removed. Known differences to ICU: ICU treats CR LF
// as one paragraph separator, unicode-bidi splits after each; line-level L1
// handling, bracket pairs and the Bidi_Class of code points new in Unicode
// 17 may differ. Arabic letter shaping (`applyArabicShaping`,
// ICU's u_shapeArabic) is a port of ICU's ushape.cpp (crate::icu::ushape).
//
// Where the C++ throws std::runtime_error, these return Err with the same
// message (`what()`).

use std::ops::Range;

use mln_vec_map::VecSet;
use unicode_bidi::ParagraphInfo;
use unicode_bidi::utf16::BidiInfo;

use crate::icu::ubidi_props::{ubidi_get_class, ubidi_get_mirror};
use crate::icu::ushape::u_shape_arabic;
pub use crate::tagged_string::StyledText;

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

/// `startsRightToLeft`: whether the first character in `text` with a
/// strong direction runs right to left.
#[must_use]
pub fn starts_right_to_left(text: &[u16]) -> bool {
    // u_charDirection: U_LEFT_TO_RIGHT, U_RIGHT_TO_LEFT,
    // U_RIGHT_TO_LEFT_ARABIC.
    const U_LEFT_TO_RIGHT: u8 = 0;
    const U_RIGHT_TO_LEFT: u8 = 1;
    const U_RIGHT_TO_LEFT_ARABIC: u8 = 13;
    for &ch in text {
        match ubidi_get_class(i32::from(ch)) {
            U_LEFT_TO_RIGHT => return false,
            U_RIGHT_TO_LEFT | U_RIGHT_TO_LEFT_ARABIC => return true,
            _ => {}
        }
    }
    false
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
        push_code_point(out, ubidi_get_mirror(c as i32) as u32);
    }
}

/// `BiDiRun`: a run of text in one direction, in logical order.
#[derive(Clone, Debug, PartialEq)]
pub struct BiDiRun {
    pub text: Vec<u16>,
    pub rtl: bool,
}

/// `BiDi`.
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

    /// `mergeParagraphLineBreaks`.
    fn merge_paragraph_line_breaks(info: &BidiInfo<'_>, line_break_points: &mut VecSet<usize>) {
        for paragraph in &info.paragraphs {
            line_break_points.insert(paragraph.range.end);
        }
    }

    /// Given text in logical ordering and a set of line break points,
    /// return a set of lines in visual order with bidi and line breaking
    /// applied.
    pub fn process_text(
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
    pub fn process_styled_text(
        &mut self,
        input: &StyledText,
        line_break_points: VecSet<usize>,
    ) -> Result<Vec<StyledText>, String> {
        self.process_styled_text_with_blocks(input, line_break_points, &input.0, &[])
    }

    /// `splitStyledText`: splits text in logical order into lines at the
    /// line break points and at the ends of paragraphs, without reordering
    /// it.
    pub fn split_styled_text(
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
    pub fn process_styled_text_with_blocks(
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

    /// `visualRuns`: splits text in logical order into runs of one
    /// direction each, and returns the runs in visual order. Each run keeps
    /// its logical order, so a shaper such as HarfBuzz can lay it out in its
    /// direction.
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

    /// `getLine`: the line in visual order, mirrored, BiDi controls
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

/// Ports test/text/bidi.test.cpp. These tests mirror the unit tests in
/// mapbox-gl-rtl-text's arabic.test.js.
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
}
