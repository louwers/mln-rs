// Float types and the order of float operations follow the C++ exactly
// (`double` section scales and line offsets, `float` positions); the
// comments note where C++ promotes to double. getShaping returns Err where
// the C++ BiDi throws (it cannot for the line breaks determineLineBreaks
// produces).

use mln_style_types::image::{ImagePosition, ImagePositions, RectU16, TextFit};
use mln_style_types::types::{IconTextFitType, SymbolAnchorType, TextJustifyType};
use mln_util::constants::ONE_EM;
use mln_util::{Padding, i18n};
use mln_vec_map::VecSet;

use crate::bidi::{BiDi, StyledText};
use crate::glyph::{
    Glyph, GlyphID, GlyphIDType, GlyphMap, GlyphMetrics, GlyphPositions, PositionedGlyph, PositionedLine, Shaping,
    WritingModeType,
};
use crate::harfbuzz::HBShapeAdjust;
use crate::tagged_string::{SectionOptions, TaggedString};

/// Zero width space that is used to suggest break points for Japanese
/// labels.
const ZWSP: u16 = 0x200b;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnchorAlignment {
    pub horizontal_align: f32,
    pub vertical_align: f32,
}

impl AnchorAlignment {
    #[must_use]
    pub fn new(horizontal: f32, vertical: f32) -> Self {
        AnchorAlignment { horizontal_align: horizontal, vertical_align: vertical }
    }

    #[must_use]
    pub fn get_anchor_alignment(anchor: SymbolAnchorType) -> AnchorAlignment {
        let mut result = AnchorAlignment::new(0.5, 0.5);

        match anchor {
            SymbolAnchorType::Right | SymbolAnchorType::TopRight | SymbolAnchorType::BottomRight => {
                result.horizontal_align = 1.0;
            }
            SymbolAnchorType::Left | SymbolAnchorType::TopLeft | SymbolAnchorType::BottomLeft => {
                result.horizontal_align = 0.0;
            }
            _ => {}
        }

        match anchor {
            SymbolAnchorType::Bottom | SymbolAnchorType::BottomLeft | SymbolAnchorType::BottomRight => {
                result.vertical_align = 1.0;
            }
            SymbolAnchorType::Top | SymbolAnchorType::TopLeft | SymbolAnchorType::TopRight => {
                result.vertical_align = 0.0;
            }
            _ => {}
        }

        result
    }
}

/// Choose the justification that matches the direction of the `TextAnchor`.
#[must_use]
pub fn get_anchor_justification(anchor: SymbolAnchorType) -> TextJustifyType {
    match anchor {
        SymbolAnchorType::Right | SymbolAnchorType::TopRight | SymbolAnchorType::BottomRight => TextJustifyType::Right,
        SymbolAnchorType::Left | SymbolAnchorType::TopLeft | SymbolAnchorType::BottomLeft => TextJustifyType::Left,
        _ => TextJustifyType::Center,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionedIcon {
    image: ImagePosition,
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
    collision_padding: Padding,
}

impl PositionedIcon {
    fn new(image: ImagePosition, top: f32, bottom: f32, left: f32, right: f32, collision_padding: Padding) -> Self {
        PositionedIcon { image, top, bottom, left, right, collision_padding }
    }

    #[must_use]
    pub fn shape_icon(image: &ImagePosition, icon_offset: &[f32; 2], icon_anchor: SymbolAnchorType) -> PositionedIcon {
        let anchor_align = AnchorAlignment::get_anchor_alignment(icon_anchor);
        let dx = icon_offset[0];
        let dy = icon_offset[1];
        let left = dx - image.display_size()[0] * anchor_align.horizontal_align;
        let right = left + image.display_size()[0];
        let top = dy - image.display_size()[1] * anchor_align.vertical_align;
        let bottom = top + image.display_size()[1];

        let mut collision_padding = Padding::default();
        if let Some(content) = &image.content {
            let pixel_ratio = image.pixel_ratio;
            collision_padding.left = content.left / pixel_ratio;
            collision_padding.top = content.top / pixel_ratio;
            collision_padding.right = image.display_size()[0] - content.right / pixel_ratio;
            collision_padding.bottom = image.display_size()[1] - content.bottom / pixel_ratio;
        }

        PositionedIcon::new(image.clone(), top, bottom, left, right, collision_padding)
    }

    /// Called after a `PositionedIcon` has already been run through
    /// fitIconToText, but needs further adjustment to apply textFitWidth
    /// and textFitHeight.
    #[must_use]
    pub fn apply_text_fit(&self) -> PositionedIcon {
        if self.image.text_fit_width.is_none() && self.image.text_fit_height.is_none() {
            return self.clone();
        }
        let width = self.right - self.left;
        let height = self.bottom - self.top;

        // C++ dereferences `content` and both text fits unchecked here.
        let content = self.image.content.expect("applyTextFit: image content");
        let content_width = content.right - content.left;
        let content_height = content.bottom - content.top;
        let content_aspect_ratio = content_width / content_height;
        let text_fit_width = self.image.text_fit_width.expect("applyTextFit: textFitWidth");
        let text_fit_height = self.image.text_fit_height.expect("applyTextFit: textFitHeight");
        let mut new_left = self.left;
        let mut new_right = self.right;
        let mut new_top = self.top;
        let mut new_bottom = self.bottom;

        if text_fit_height == TextFit::Proportional {
            if (text_fit_width == TextFit::StretchOnly && (width / height) < content_aspect_ratio)
                || text_fit_width == TextFit::Proportional
            {
                let new_icon_width = (height * content_aspect_ratio).ceil();
                new_left *= new_icon_width / width;
                new_right = new_left + new_icon_width;
            }
        } else if text_fit_width == TextFit::Proportional {
            if text_fit_height == TextFit::StretchOnly
                && content_aspect_ratio != 0.0
                && (width / height) > content_aspect_ratio
            {
                let new_icon_height = (width / content_aspect_ratio).ceil();
                new_top *= new_icon_height / height;
                new_bottom = new_top + new_icon_height;
            }
        } else {
            // If neither textFitHeight nor textFitWidth are proportional
            // then there is no effect since the content rectangle should be
            // precisely matched to the content
        }
        PositionedIcon::new(self.image.clone(), new_top, new_bottom, new_left, new_right, self.collision_padding)
    }

    /// Updates shaped icon's bounds based on shaped text's bounds and
    /// provided layout properties.
    pub fn fit_icon_to_text(
        &mut self,
        shaped_text: &Shaping,
        text_fit: IconTextFitType,
        padding: &[f32; 4],
        icon_offset: &[f32; 2],
        font_scale: f32,
    ) {
        debug_assert_ne!(text_fit, IconTextFitType::None);
        debug_assert!(shaped_text.is_some());

        // We don't respect the icon-anchor, because icon-text-fit is set.
        // Instead, the icon will be centered on the text, then stretched in
        // the given dimensions.

        let text_left = shaped_text.left * font_scale;
        let text_right = shaped_text.right * font_scale;

        if text_fit == IconTextFitType::Width || text_fit == IconTextFitType::Both {
            // Stretched horizontally to the text width
            self.left = icon_offset[0] + text_left - padding[3];
            self.right = icon_offset[0] + text_right + padding[1];
        } else {
            // Centered on the text
            self.left = icon_offset[0] + (text_left + text_right - self.image.display_size()[0]) / 2.0;
            self.right = self.left + self.image.display_size()[0];
        }

        let text_top = shaped_text.top * font_scale;
        let text_bottom = shaped_text.bottom * font_scale;
        if text_fit == IconTextFitType::Height || text_fit == IconTextFitType::Both {
            // Stretched vertically to the text height
            self.top = icon_offset[1] + text_top - padding[0];
            self.bottom = icon_offset[1] + text_bottom + padding[2];
        } else {
            // Centered on the text
            self.top = icon_offset[1] + (text_top + text_bottom - self.image.display_size()[1]) / 2.0;
            self.bottom = self.top + self.image.display_size()[1];
        }
    }

    #[must_use]
    pub fn image(&self) -> &ImagePosition {
        &self.image
    }
    #[must_use]
    pub fn top(&self) -> f32 {
        self.top
    }
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.bottom
    }
    #[must_use]
    pub fn left(&self) -> f32 {
        self.left
    }
    #[must_use]
    pub fn right(&self) -> f32 {
        self.right
    }
    #[must_use]
    pub fn collision_padding(&self) -> &Padding {
        &self.collision_padding
    }
}

/// `align`.
#[allow(clippy::too_many_arguments)]
fn align(
    shaping: &mut Shaping,
    justify: f32,
    horizontal_align: f32,
    vertical_align: f32,
    max_line_length: f32,
    max_line_height: f32,
    line_height: f32,
    block_height: f32,
    line_count: usize,
) {
    let shift_x = (justify - horizontal_align) * max_line_length;
    let shift_y = if max_line_height == line_height {
        (-vertical_align * line_count as f32 + 0.5) * line_height
    } else {
        -block_height * vertical_align - Shaping::Y_OFFSET as f32
    };

    for line in &mut shaping.positioned_lines {
        for positioned_glyph in &mut line.positioned_glyphs {
            positioned_glyph.x += shift_x;
            positioned_glyph.y += shift_y;
        }
    }
}

/// Shifts the glyphs by `justify` of the line's width (left = 0, right = 1,
/// center = 0.5), and vertically by `line_offset`.
fn justify_line(positioned_glyphs: &mut [PositionedGlyph], justify: f32, line_offset: f32) {
    if justify == 0.0 && line_offset == 0.0 {
        return;
    }

    let last_glyph = positioned_glyphs.last().expect("justifyLine on an empty line");
    let last_advance = last_glyph.metrics.advance as f32 * last_glyph.scale;
    let line_indent = (last_glyph.x + last_advance) * justify;
    for positioned_glyph in positioned_glyphs.iter_mut() {
        positioned_glyph.x -= line_indent;
        positioned_glyph.y += line_offset;
    }
}

fn get_glyph_advance(
    code_point: u16,
    section: &SectionOptions,
    glyph_map: &GlyphMap,
    image_positions: &ImagePositions,
    layout_text_size: f32,
    spacing: f32,
) -> f32 {
    match &section.image_id {
        None => {
            let Some(glyphs) = glyph_map.get(&section.font_stack_hash) else {
                return 0.0;
            };
            match glyphs.get(&GlyphID::from(code_point)) {
                Some(Some(glyph)) => (f64::from(glyph.metrics.advance) * section.scale) as f32 + spacing,
                _ => 0.0,
            }
        }
        Some(image_id) => {
            let Some(image) = image_positions.get(image_id) else {
                return 0.0;
            };
            image.display_size()[0] * section.scale as f32 * ONE_EM / layout_text_size + spacing
        }
    }
}

fn determine_average_line_width(
    logical_input: &TaggedString,
    spacing: f32,
    max_width: f32,
    glyph_map: &GlyphMap,
    image_positions: &ImagePositions,
    layout_text_size: f32,
) -> f32 {
    let mut total_width: f32 = 0.0;

    for i in 0..logical_input.length() {
        let section = logical_input.get_section(i);
        if section.type_ != GlyphIDType::FontPBF {
            continue;
        }
        let code_point = logical_input.get_char_code_at(i);
        total_width += get_glyph_advance(code_point, section, glyph_map, image_positions, layout_text_size, spacing);
    }

    // static_cast<int32_t>(::fmax(1, std::ceil(totalWidth / maxWidth)))
    // (an infinite quotient is UB in C++; `as` saturates)
    let target_line_count = mln_math::fmax(1.0, f64::from((total_width / max_width).ceil())) as i32;
    total_width / target_line_count as f32
}

fn calculate_badness(line_width: f32, target_width: f32, penalty: f32, is_last_break: bool) -> f32 {
    // static_cast<float>(std::pow(lineWidth - targetWidth, 2)): pow of the
    // float difference in double, which is exact for a square.
    let d = f64::from(line_width - target_width);
    let raggedness = (d * d) as f32;
    if is_last_break {
        // Favor finals lines shorter than average over longer than average
        if line_width < target_width {
            return raggedness / 2.0;
        }
        return raggedness * 2.0;
    }
    if penalty < 0.0 {
        return raggedness - penalty * penalty;
    }
    raggedness + penalty * penalty
}

fn calculate_penalty(code_point: u16, next_code_point: u16, penalizable_ideographic_break: bool) -> f32 {
    let mut penalty: f32 = 0.0;
    // Force break on newline
    if code_point == 0x0a {
        penalty -= 10000.0;
    }

    // Penalize open parenthesis at end of line
    if code_point == 0x28 || code_point == 0xff08 {
        penalty += 50.0;
    }

    // Penalize close parenthesis at beginning of line
    if next_code_point == 0x29 || next_code_point == 0xff09 {
        penalty += 50.0;
    }

    // Penalize breaks between characters that allow ideographic breaking
    // because they are less preferable than breaks at spaces (or zero width
    // spaces)
    if penalizable_ideographic_break {
        penalty += 150.0;
    }

    penalty
}

/// A line break candidate. `prior_break` is an index into the list.
#[derive(Clone, Copy, Debug)]
struct PotentialBreak {
    index: usize,
    x: f32,
    prior_break: Option<usize>,
    badness: f32,
}

fn evaluate_break(
    break_index: usize,
    break_x: f32,
    target_width: f32,
    potential_breaks: &[PotentialBreak],
    penalty: f32,
    is_last_break: bool,
) -> PotentialBreak {
    // We could skip evaluating breaks where the line length (breakX -
    // priorBreak.x) > maxWidth ...but in fact we allow lines longer than
    // maxWidth (if there's no break points) ...and when targetWidth and
    // maxWidth are close, strictly enforcing maxWidth can give more
    // lopsided results.

    let mut best_prior_break = None;
    let mut best_break_badness = calculate_badness(break_x, target_width, penalty, is_last_break);
    for (k, potential_break) in potential_breaks.iter().enumerate() {
        let line_width = break_x - potential_break.x;
        let break_badness =
            calculate_badness(line_width, target_width, penalty, is_last_break) + potential_break.badness;
        if break_badness <= best_break_badness {
            best_prior_break = Some(k);
            best_break_badness = break_badness;
        }
    }

    PotentialBreak { index: break_index, x: break_x, prior_break: best_prior_break, badness: best_break_badness }
}

fn least_bad_breaks(last_line_break: &PotentialBreak, potential_breaks: &[PotentialBreak]) -> VecSet<usize> {
    let mut least_bad_breaks = VecSet::from([last_line_break.index]);
    let mut prior_break = last_line_break.prior_break;
    while let Some(k) = prior_break {
        least_bad_breaks.insert(potential_breaks[k].index);
        prior_break = potential_breaks[k].prior_break;
    }
    least_bad_breaks
}

/// Determines line breaks based on shaped text in logical order. Working in
/// visual order would be more intuitive, but we can't do that because the
/// visual order may be changed by line breaks!
#[must_use]
pub fn determine_line_breaks(
    logical_input: &TaggedString,
    spacing: f32,
    max_width: f32,
    glyph_map: &GlyphMap,
    image_positions: &ImagePositions,
    layout_text_size: f32,
) -> VecSet<usize> {
    if max_width == 0.0 {
        return VecSet::new();
    }

    if logical_input.empty() {
        return VecSet::new();
    }

    let target_width =
        determine_average_line_width(logical_input, spacing, max_width, glyph_map, image_positions, layout_text_size);

    let mut potential_breaks: Vec<PotentialBreak> = Vec::new();
    let mut current_x: f32 = 0.0;
    // Find first occurrence of zero width space (ZWSP) character.
    let has_server_suggested_breaks = logical_input.raw_text().contains(&ZWSP);

    let length = logical_input.length();
    for i in 0..length {
        let section = logical_input.get_section(i);
        let code_point = logical_input.get_char_code_at(i);
        if !i18n::is_whitespace(code_point) {
            current_x += get_glyph_advance(code_point, section, glyph_map, image_positions, layout_text_size, spacing);
        }

        // Ideographic characters, spaces, and word-breaking punctuation
        // that often appear without surrounding spaces.
        if i < length - 1 {
            let allows_ideographic_break = i18n::allows_ideographic_breaking(code_point);
            let allows_break_before_next =
                i != length - 2 && i18n::allows_word_breaking_before(logical_input.get_char_code_at(i + 1));
            if section.image_id.is_some()
                || allows_ideographic_break
                || i18n::allows_word_breaking(code_point)
                || allows_break_before_next
            {
                let penalizable_ideographic_break = allows_ideographic_break && has_server_suggested_breaks;
                let next_index = i + 1;
                let b = evaluate_break(
                    next_index,
                    current_x,
                    target_width,
                    &potential_breaks,
                    calculate_penalty(
                        code_point,
                        logical_input.get_char_code_at(next_index),
                        penalizable_ideographic_break,
                    ),
                    false,
                );
                potential_breaks.push(b);
            }
        }
    }

    let last = evaluate_break(length, current_x, target_width, &potential_breaks, 0.0, true);
    least_bad_breaks(&last, &potential_breaks)
}

const MAX_UPRIGHT_LETTER_RUN: usize = 3;

/// Whether the run `start..end` stays upright in vertical text. Longer
/// uppercase runs are words, while short runs such as "JR" and "A1" are
/// codes.
fn run_is_upright(line: &TaggedString, start: usize, end: usize) -> bool {
    let mut has_digit = false;
    let mut is_number = true;
    let mut is_short_uppercase_code = end - start <= MAX_UPRIGHT_LETTER_RUN;
    for i in start..end {
        let chr = line.get_char_code_at(i);
        let is_digit = i18n::is_digit(chr);
        has_digit |= is_digit;
        is_number &= is_digit || i18n::is_punctuation_or_symbol(chr);
        is_short_uppercase_code &= is_digit || i18n::is_uppercase(chr);
    }
    (has_digit && is_number) || is_short_uppercase_code
}

/// Whether the character at `i` is text rather than an image or a shaped
/// glyph.
fn is_text(line: &TaggedString, i: usize) -> bool {
    let section = line.get_section(i);
    section.type_ == GlyphIDType::FontPBF && section.image_id.is_none()
}

/// Punctuation such as the hyphen in "1-2" needs a vertical presentation
/// form now that its neighbors are upright. The glyph is replaced during
/// shaping.
fn mark_surrounded_punctuation_upright(line: &TaggedString, verticals: &mut [bool]) {
    for i in 0..line.length() {
        if !verticals[i]
            && is_text(line, i)
            && i18n::verticalize_punctuation(line.get_char_code_at(i)) != 0
            && (i == 0 || verticals[i - 1])
            && (i + 1 == line.length() || verticals[i + 1])
        {
            verticals[i] = true;
        }
    }
}

/// Which characters of `line` are upright in vertical text. Keeps words and
/// combining marks together; images and shaped glyph IDs delimit runs.
fn determine_line_verticals(line: &TaggedString) -> Vec<bool> {
    let mut verticals = vec![false; line.length()];
    for (i, vertical) in verticals.iter_mut().enumerate() {
        *vertical = is_text(line, i) && i18n::has_upright_vertical_orientation(line.get_char_code_at(i));
    }

    let is_run_character = |verticals: &[bool], i: usize| {
        is_text(line, i) && !verticals[i] && !i18n::is_whitespace(line.get_char_code_at(i))
    };
    let mut start = 0;
    while start < line.length() {
        if !is_run_character(&verticals, start) {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < line.length() && is_run_character(&verticals, end) {
            end += 1;
        }
        if run_is_upright(line, start, end) {
            for (i, vertical) in verticals.iter_mut().enumerate().take(end).skip(start) {
                let chr = line.get_char_code_at(i);
                *vertical =
                    (i18n::is_uppercase(chr) || i18n::is_digit(chr)) && !i18n::is_char_in_complex_shaping_script(chr);
            }
        }
        start = end - 1;
        start += 1;
    }

    mark_surrounded_punctuation_upright(line, &mut verticals);
    verticals
}

#[allow(clippy::too_many_arguments)]
fn shape_lines(
    shaping: &mut Shaping,
    lines: &mut [TaggedString],
    spacing: f32,
    line_height: f32,
    text_anchor: SymbolAnchorType,
    text_justify: TextJustifyType,
    writing_mode: WritingModeType,
    glyph_map: &GlyphMap,
    glyph_positions: &GlyphPositions,
    image_positions: &ImagePositions,
    layout_text_size: f32,
    allow_vertical_placement: bool,
) {
    let mut x: f32 = 0.0;
    let mut y: f32 = Shaping::Y_OFFSET as f32;

    let mut max_line_length: f32 = 0.0;
    let mut max_line_height: f32 = 0.0;

    let justify: f32 = if text_justify == TextJustifyType::Right {
        1.0
    } else if text_justify == TextJustifyType::Left {
        0.0
    } else {
        0.5
    };

    for line in lines.iter_mut() {
        // Collapse whitespace so it doesn't throw off justification
        line.trim();

        let line_max_scale: f64 = line.get_max_scale();
        let max_line_offset: f64 = (line_max_scale - 1.0) * f64::from(ONE_EM);
        let mut line_offset: f64 = 0.0;
        shaping.positioned_lines.push(PositionedLine::default());
        let line_index = shaping.positioned_lines.len() - 1;

        if line.empty() {
            y += line_height; // Still need a line feed after empty line
            continue;
        }

        let line_verticals = if writing_mode == WritingModeType::Vertical && !allow_vertical_placement {
            determine_line_verticals(line)
        } else {
            Vec::new()
        };

        for i in 0..line.length() {
            let section_index = line.get_section_index(i) as usize;
            let section = line.section_at(section_index);
            let mut adjust: Option<&HBShapeAdjust> = None;
            if let Some(adjusts) = &section.adjusts {
                debug_assert!(section.start_index >= 0);
                // std::cmp_greater_equal(i, startIndex) && i - startIndex <
                // adjusts->size() (size_t arithmetic)
                let offset = (i as u64).wrapping_sub(i64::from(section.start_index) as u64);
                if (section.start_index < 0 || i as u64 >= section.start_index as u64) && offset < adjusts.len() as u64
                {
                    adjust = Some(&adjusts[offset as usize]);
                }
            }
            let mut code_point = GlyphID::with_type(line.get_char_code_at(i), section.type_);

            // (double baselineOffset = 0.0, float advance = 0.0f: every
            // path that reaches the positioned glyph assigns both)
            let baseline_offset: f64;
            let mut rect = RectU16::default();
            let mut metrics = GlyphMetrics::default();
            let mut advance: f32;
            let mut x_hb_offset: f32 = 0.0;
            let mut y_hb_offset: f32 = 0.0;
            let mut vertical_advance: f32 = ONE_EM;
            let mut section_scale: f64 = section.scale;
            debug_assert!(section_scale != 0.0);

            let vertical = if line_verticals.is_empty() {
                writing_mode != WritingModeType::Horizontal
                    && code_point.type_ == GlyphIDType::FontPBF
                    && (if allow_vertical_placement {
                        !i18n::is_whitespace(code_point.code)
                            && !i18n::is_char_in_complex_shaping_script(code_point.code)
                    } else {
                        i18n::has_upright_vertical_orientation(code_point.code)
                    })
            } else {
                line_verticals[i]
            };
            if !line_verticals.is_empty() && vertical && !i18n::has_upright_vertical_orientation(code_point.code) {
                let replacement = i18n::verticalize_punctuation(code_point.code);
                if replacement != 0 {
                    code_point = GlyphID::with_type(replacement, section.type_);
                }
            }

            match &section.image_id {
                None => {
                    let Some(glyph_position_map) = glyph_positions.get(&section.font_stack_hash) else {
                        continue;
                    };

                    if let Some(glyph_position) = glyph_position_map.get(&code_point) {
                        rect = glyph_position.rect;
                        metrics = glyph_position.metrics;
                    } else {
                        let Some(glyphs) = glyph_map.get(&section.font_stack_hash) else {
                            continue;
                        };

                        let Some(Some(glyph)) = glyphs.get(&code_point) else {
                            continue;
                        };
                        metrics = glyph.metrics;
                    }
                    advance = metrics.advance as f32;

                    if let Some(adjust) = adjust {
                        advance = adjust.advance;
                        x_hb_offset = (f64::from(adjust.x_offset) * section.scale) as f32;
                        y_hb_offset = (f64::from(adjust.y_offset) * section.scale) as f32;
                    }
                    if advance < 0.01 {
                        // Advance is 0, this glyph should align to the
                        // preview glyph remove spacing
                        x_hb_offset -= spacing;
                    }

                    // We don't know the baseline, but since we're laying
                    // out at 24 points, we can calculate how much it will
                    // move when we scale up or down.
                    baseline_offset = (line_max_scale - section_scale) * f64::from(ONE_EM);
                }
                Some(image_id) => {
                    let Some(image) = image_positions.get(image_id) else {
                        continue;
                    };
                    shaping.icons_in_text |= true;
                    let display_size = image.display_size();
                    // static_cast<uint32_t>: a negative display size (an image narrower
                    // than its padding) is UB in C++; `as` saturates to 0.
                    metrics.width = display_size[0] as u32;
                    metrics.height = display_size[1] as u32;
                    metrics.left = f32::from(ImagePosition::PADDING);
                    metrics.top = -f32::from(Glyph::BORDER_SIZE);
                    metrics.advance = if vertical { metrics.height } else { metrics.width };
                    rect = image.padded_rect;

                    // If needed, allow to set scale factor for an image
                    // using alias "image-scale" that could be alias for
                    // "font-scale" when FormattedSection is an image
                    // section.
                    section_scale = section_scale * f64::from(ONE_EM) / f64::from(layout_text_size);

                    // Difference between one EM and an image size. Aligns
                    // bottom of an image to a baseline level.
                    let image_offset: f32 = ONE_EM - display_size[1] * section_scale as f32;
                    baseline_offset = max_line_offset + f64::from(image_offset);
                    vertical_advance = metrics.advance as f32;
                    advance = vertical_advance;

                    // Difference between height of an image and one EM at
                    // max line scale. Pushes current line down if an image
                    // size is over 1 EM at max line scale.
                    let offset: f64 = f64::from(if vertical { display_size[0] } else { display_size[1] })
                        * section_scale
                        - f64::from(ONE_EM) * line_max_scale;
                    if offset > 0.0 && offset > line_offset {
                        line_offset = offset;
                    }
                }
            }

            let positioned_glyphs = &mut shaping.positioned_lines[line_index].positioned_glyphs;
            if vertical {
                positioned_glyphs.push(PositionedGlyph::new(
                    code_point,
                    x,
                    y + baseline_offset as f32,
                    vertical,
                    section.font_stack_hash,
                    section_scale as f32,
                    rect,
                    metrics,
                    section.image_id.clone(),
                    section_index,
                ));
                x += vertical_advance * section_scale as f32 + spacing;
                shaping.verticalizable |= true;
            } else {
                positioned_glyphs.push(PositionedGlyph::new(
                    code_point,
                    x + x_hb_offset,
                    y + baseline_offset as f32 + y_hb_offset,
                    vertical,
                    section.font_stack_hash,
                    section_scale as f32,
                    rect,
                    metrics,
                    section.image_id.clone(),
                    section_index,
                ));
                if advance > 0.01 {
                    // Only thce glyph with advance should increase spacing
                    x += advance * section_scale as f32 + spacing;
                }
            }
        }

        // Only justify if we placed at least one glyph
        let positioned_glyphs = &mut shaping.positioned_lines[line_index].positioned_glyphs;
        if !positioned_glyphs.is_empty() {
            let line_length = x - spacing; // Don't count trailing spacing
            max_line_length = mln_math::fmaxf(line_length, max_line_length); // util::max
            justify_line(positioned_glyphs, justify, line_offset as f32);
        }

        let current_line_height = (f64::from(line_height) * line_max_scale + line_offset) as f32;
        x = 0.0;
        y += current_line_height;
        // std::max(lineOffset, maxLineOffset)
        shaping.positioned_lines[line_index].line_offset =
            (if line_offset < max_line_offset { max_line_offset } else { line_offset }) as f32;
        // std::max(currentLineHeight, maxLineHeight)
        max_line_height = if current_line_height < max_line_height { max_line_height } else { current_line_height };
    }

    let anchor_align = AnchorAlignment::get_anchor_alignment(text_anchor);
    let height = y - Shaping::Y_OFFSET as f32;
    align(
        shaping,
        justify,
        anchor_align.horizontal_align,
        anchor_align.vertical_align,
        max_line_length,
        max_line_height,
        line_height,
        height,
        lines.len(),
    );

    // Calculate the bounding box
    shaping.top += -anchor_align.vertical_align * height;
    shaping.bottom = shaping.top + height;
    shaping.left += -anchor_align.horizontal_align * max_line_length;
    shaping.right = shaping.left + max_line_length;
}

/// Reorders lines that hold text HarfBuzz shaped, which come in logical
/// order, into visual order. The bidi analysis covers the whole text, and
/// sees each shaped section as characters of the section's direction, which
/// it moves as a block without reversing them again.
fn reorder_shaped_lines(
    formatted_string: &TaggedString,
    bidi: &mut BiDi,
    lines: &mut Vec<TaggedString>,
) -> Result<(), String> {
    let sections = formatted_string.get_sections();
    let styled_text = formatted_string.get_styled_text();

    let mut bidi_text = styled_text.0.clone();
    for (i, ch) in bidi_text.iter_mut().enumerate() {
        let section = &sections[styled_text.1[i] as usize];
        if section.type_ != GlyphIDType::FontPBF {
            *ch = if section.rtl { 0x05D0 } else { 'a' as u16 };
        }
    }
    let block_styles: Vec<bool> = sections.iter().map(|section| section.type_ != GlyphIDType::FontPBF).collect();

    let mut line_breaks = VecSet::new();
    let mut line_end = 0;
    for line in lines.iter() {
        line_end += line.length();
        line_breaks.insert(line_end);
    }

    lines.clear();
    for line in bidi.process_styled_text_with_blocks(styled_text, line_breaks, &bidi_text, &block_styles)? {
        // A shaped section's adjustments start where its block now starts.
        let mut line_sections = sections.to_vec();
        for (i, line_section) in line_sections.iter_mut().enumerate() {
            if !block_styles[i] {
                continue;
            }
            let start = line.1.iter().position(|&style| style as usize == i).unwrap_or(line.1.len());
            line_section.start_index = start as i32;
        }
        lines.push(TaggedString::from_styled_text(line, line_sections));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn get_shaping(
    formatted_string: &TaggedString,
    max_width: f32,
    line_height: f32,
    text_anchor: SymbolAnchorType,
    text_justify: TextJustifyType,
    spacing: f32,
    translate: &[f32; 2],
    writing_mode: WritingModeType,
    bidi: &mut BiDi,
    glyph_map: &GlyphMap,
    glyph_positions: &GlyphPositions,
    image_positions: &ImagePositions,
    layout_text_size: f32,
    layout_text_size_at_bucket_zoom_level: f32,
    allow_vertical_placement: bool,
) -> Result<Shaping, String> {
    debug_assert!(layout_text_size != 0.0);
    let mut reordered_lines: Vec<TaggedString> = Vec::new();
    if !formatted_string.raw_text().is_empty() {
        if formatted_string.section_count() == 1 {
            if formatted_string.get_section(0).type_ == GlyphIDType::FontPBF {
                let untagged_lines = bidi.process_text(
                    formatted_string.raw_text(),
                    determine_line_breaks(
                        formatted_string,
                        spacing,
                        max_width,
                        glyph_map,
                        image_positions,
                        layout_text_size,
                    ),
                )?;
                for line in untagged_lines {
                    reordered_lines.push(TaggedString::new(line, formatted_string.section_at(0).clone()));
                }
            } else {
                reordered_lines.push(formatted_string.clone());
            }
        } else {
            let mut sub_string: StyledText = (Vec::new(), Vec::new());
            let mut section_type = GlyphIDType::FontPBF;
            let str_len = formatted_string.get_styled_text().0.len();

            let mut formatted_sections: Vec<SectionOptions> = formatted_string.get_sections().to_vec();
            if !formatted_sections.is_empty() {
                section_type = formatted_sections[0].type_;
            }

            // Text that HarfBuzz shaped is already in visual order, so bidi
            // can't reorder it a character at a time. The loop below then
            // only splits the text into lines, and the whole text is
            // reordered after.
            let has_shaped_sections = formatted_sections.iter().any(|section| section.type_ != GlyphIDType::FontPBF);

            let mut pend_strings: Vec<StyledText> = Vec::new();

            let process_aline = |line: StyledText,
                                 formatted_sections: &mut Vec<SectionOptions>,
                                 reordered_lines: &mut Vec<TaggedString>| {
                let cut_lens = line.0.len() as i32;
                reordered_lines.push(TaggedString::from_styled_text(line, formatted_sections.clone()));

                for sec in formatted_sections.iter_mut() {
                    sec.start_index = sec.start_index.wrapping_sub(cut_lens);
                }
            };

            let apply_line_and_pending_strings =
                |line: StyledText,
                 pend_strings: &mut Vec<StyledText>,
                 formatted_sections: &mut Vec<SectionOptions>,
                 reordered_lines: &mut Vec<TaggedString>| {
                    if pend_strings.is_empty() {
                        process_aline(line, formatted_sections, reordered_lines);
                    } else {
                        let mut combine: StyledText = (Vec::new(), Vec::new());
                        for pend_string in pend_strings.iter() {
                            combine.0.extend_from_slice(&pend_string.0);
                            combine.1.extend_from_slice(&pend_string.1);
                        }
                        pend_strings.clear();
                        combine.0.extend_from_slice(&line.0);
                        combine.1.extend_from_slice(&line.1);
                        process_aline(combine, formatted_sections, reordered_lines);
                    }
                };

            let mut apply_sub_string = |sub_string: &StyledText,
                                        section_type: GlyphIDType,
                                        pend_strings: &mut Vec<StyledText>,
                                        formatted_sections: &mut Vec<SectionOptions>,
                                        reordered_lines: &mut Vec<TaggedString>|
             -> Result<(), String> {
                if !sub_string.0.is_empty() {
                    if GlyphIDType::FontPBF == section_type {
                        let line_breaks = determine_line_breaks(
                            &TaggedString::from_styled_text(
                                sub_string.clone(),
                                formatted_string.get_sections().to_vec(),
                            ),
                            spacing,
                            max_width,
                            glyph_map,
                            image_positions,
                            layout_text_size,
                        );
                        let processed_lines = if has_shaped_sections {
                            bidi.split_styled_text(sub_string, line_breaks)?
                        } else {
                            bidi.process_styled_text(sub_string, line_breaks)?
                        };

                        let mut last_char: u16 = 'x' as u16;
                        if !sub_string.0.is_empty() {
                            last_char = sub_string.0[sub_string.0.len() - 1];
                        }

                        if '\n' as u16 == last_char {
                            for line in processed_lines {
                                apply_line_and_pending_strings(line, pend_strings, formatted_sections, reordered_lines);
                            }
                        } else {
                            let line_count = processed_lines.len();
                            let mut processed_lines = processed_lines.into_iter();
                            if line_count > 1 {
                                for _ in 0..line_count - 1 {
                                    let line = processed_lines.next().unwrap();
                                    apply_line_and_pending_strings(
                                        line,
                                        pend_strings,
                                        formatted_sections,
                                        reordered_lines,
                                    );
                                }
                            }
                            if line_count != 0 {
                                pend_strings.push(processed_lines.next().unwrap());
                            }
                        }
                    } else {
                        pend_strings.push(sub_string.clone());
                    }
                }
                Ok(())
            };

            for char_index in 0..str_len {
                let ch = formatted_string.get_styled_text().0[char_index];
                let sec = formatted_string.get_styled_text().1[char_index];
                let sec_type = formatted_sections[sec as usize].type_;

                if section_type != sec_type {
                    apply_sub_string(
                        &sub_string,
                        section_type,
                        &mut pend_strings,
                        &mut formatted_sections,
                        &mut reordered_lines,
                    )?;

                    sub_string.0.clear();
                    sub_string.1.clear();

                    section_type = sec_type;
                }

                sub_string.0.push(ch);
                sub_string.1.push(sec);
            }

            apply_sub_string(
                &sub_string,
                section_type,
                &mut pend_strings,
                &mut formatted_sections,
                &mut reordered_lines,
            )?;

            if !pend_strings.is_empty() {
                let mut combine: StyledText = (Vec::new(), Vec::new());
                for pend_string in &pend_strings {
                    combine.0.extend_from_slice(&pend_string.0);
                    combine.1.extend_from_slice(&pend_string.1);
                }
                pend_strings.clear();
                process_aline(combine, &mut formatted_sections, &mut reordered_lines);
            }

            if has_shaped_sections {
                reorder_shaped_lines(formatted_string, bidi, &mut reordered_lines)?;
            }
        }
    }

    let mut shaping = Shaping::new(translate[0], translate[1], writing_mode);
    shape_lines(
        &mut shaping,
        &mut reordered_lines,
        spacing,
        line_height,
        text_anchor,
        text_justify,
        writing_mode,
        glyph_map,
        glyph_positions,
        image_positions,
        layout_text_size_at_bucket_zoom_level,
        allow_vertical_placement,
    );

    Ok(shaping)
}
