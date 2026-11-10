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

use std::collections::BTreeSet;
use std::ops::Range;

use unicode_bidi::ParagraphInfo;
use unicode_bidi::utf16::BidiInfo;

use crate::icu::ubidi_props::ubidi_get_mirror;
use crate::icu::ushape::u_shape_arabic;
pub use crate::tagged_string::StyledText;

/// Takes UTF16 input in logical order and applies Arabic shaping to the
/// input while maintaining logical order. Output won't be intelligible
/// until the bidirectional algorithm is applied.
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

/// `IS_BIDI_CONTROL_CHAR(c)` (ICU ubidiimp.h): what
/// UBIDI_REMOVE_BIDI_CONTROLS removes.
fn is_bidi_control_char(c: u16) -> bool {
    let c = c as u32;
    (c & 0xfffffffc) == 0x200c || c.wrapping_sub(0x202a) < 5 || c.wrapping_sub(0x2066) < 4
}

/// The code point ending at `text[end - 1]` and its first index.
fn code_point_before(text: &[u16], end: usize) -> (u32, usize) {
    let c = text[end - 1] as u32;
    if (c & 0xfc00) == 0xdc00 && end >= 2 {
        let lead = text[end - 2] as u32;
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
/// UBIDI_DO_MIRRORING | UBIDI_REMOVE_BIDI_CONTROLS: code points in reverse
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
    pub fn new() -> Self {
        BiDi {}
    }

    /// `mergeParagraphLineBreaks`.
    fn merge_paragraph_line_breaks(info: &BidiInfo<'_>, line_break_points: &mut BTreeSet<usize>) {
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
        mut line_break_points: BTreeSet<usize>,
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
        mut line_break_points: BTreeSet<usize>,
    ) -> Result<Vec<StyledText>, String> {
        let mut lines = Vec::new();
        let input_text = &input.0;
        let style_indices = &input.1;

        let info = BidiInfo::new(input_text, None);

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
                    // Quirk kept: when the style changes at logicalStart
                    // itself, that character is written with the previous
                    // chunk's style (styleRunEnd = i).
                    let mut style_run_start = logical_end;
                    let mut current_style_index = style_indices[style_run_start - 1];
                    let mut i = logical_end - 1;
                    while i >= logical_start {
                        if current_style_index != style_indices[i] || i == logical_start {
                            let style_run_end = if i == logical_start { i } else { i + 1 };
                            let mut reversed = Vec::new();
                            write_reverse_mirrored(&input_text[style_run_end..style_run_start], &mut reversed);
                            line.0.extend_from_slice(&reversed);
                            line.1.extend(std::iter::repeat_n(current_style_index, reversed.len()));
                            current_style_index = style_indices[i];
                            style_run_start = style_run_end;
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

    fn set(v: &[usize]) -> BTreeSet<usize> {
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
}
