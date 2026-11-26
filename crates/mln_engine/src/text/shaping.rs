//! Label shaping: breaking a label into lines, placing each line's glyphs
//! and inline images, and aligning the lines on the label's anchor.
//!
//! Positions are in pixels at a font size of `ONE_EM`, with y down.

use std::ops::Range;

use glam::Vec2;
use mln_style::i18n;
use mln_style::image::TextFit;
use mln_style::padding::Padding;
use mln_style::types::{IconTextFitType, SymbolAnchorType, TextJustifyType, TextWritingModeType, VerticalAlign};
use mln_vec_map::VecSet;

use crate::text::atlas::{ImagePosition, ImagePositions};
use crate::text::bidi;
use crate::text::constants::ONE_EM;
use crate::text::glyph::{
    Bounds, Glyph, GlyphID, GlyphIDType, GlyphMetrics, GlyphPositions, PositionedGlyph, PositionedLine, Shaping,
    pbf_code,
};
use crate::text::line_breaks::determine_line_breaks;
use crate::text::shaper::GlyphAdjust;
use crate::text::tagged_string::{SectionIndex, SectionOptions, StyledText, TaggedString};

/// How [`shape_text`] lays out a label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapingOptions {
    /// The width that lines break at where they can; 0 keeps the text on
    /// one line.
    pub max_width: f32,
    /// The distance between lines of `ONE_EM` glyphs.
    pub line_height: f32,
    /// The point of the text that sits on the anchor.
    pub anchor: SymbolAnchorType,
    /// Which side lines align on; `Auto` centers them.
    pub justify: TextJustifyType,
    /// The space after each glyph that advances the pen.
    pub spacing: f32,
    /// How far the text's bounds move from the anchor.
    pub offset: [f32; 2],
    pub writing_mode: TextWritingModeType,
    /// Whether the layer places labels vertically (`text-writing-mode`). In
    /// vertical text, every glyph but whitespace and complex scripts then
    /// stands upright; otherwise, as in a line label along a vertical line,
    /// only the characters that stay upright in vertical writing do, with
    /// runs of digits and short uppercase codes.
    pub allow_vertical_placement: bool,
    /// The text size that line breaking measures inline images at.
    pub text_size: f32,
    /// The text size that inline images scale against, at the bucket's zoom.
    pub text_size_at_bucket_zoom: f32,
}

/// Where `anchor` sits on a box: the fraction of the box's width left of the
/// anchor, and of its height above it.
#[must_use]
pub fn anchor_alignment(anchor: SymbolAnchorType) -> Vec2 {
    use SymbolAnchorType as A;
    let x = match anchor {
        A::Left | A::TopLeft | A::BottomLeft => 0.0,
        A::Right | A::TopRight | A::BottomRight => 1.0,
        A::Center | A::Top | A::Bottom => 0.5,
    };
    let y = match anchor {
        A::Top | A::TopLeft | A::TopRight => 0.0,
        A::Bottom | A::BottomLeft | A::BottomRight => 1.0,
        A::Center | A::Left | A::Right => 0.5,
    };
    Vec2::new(x, y)
}

/// The justification that aligns lines on the side of the text that
/// `anchor` is on.
#[must_use]
pub fn anchor_justification(anchor: SymbolAnchorType) -> TextJustifyType {
    use SymbolAnchorType as A;
    match anchor {
        A::Right | A::TopRight | A::BottomRight => TextJustifyType::Right,
        A::Left | A::TopLeft | A::BottomLeft => TextJustifyType::Left,
        A::Center | A::Top | A::Bottom => TextJustifyType::Center,
    }
}

/// The fraction of a line's width that `justify` moves it left by.
fn justify_factor(justify: TextJustifyType) -> f32 {
    match justify {
        TextJustifyType::Left => 0.0,
        TextJustifyType::Right => 1.0,
        TextJustifyType::Center | TextJustifyType::Auto => 0.5,
    }
}

/// An icon placed around its anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionedIcon {
    pub image: ImagePosition,
    pub bounds: Bounds,
    /// How far in from each edge of the image its content starts, where
    /// the image has content; collisions count only the content.
    pub collision_padding: Padding,
}

impl PositionedIcon {
    /// `image` offset by `offset`, with `anchor` on the anchor.
    #[must_use]
    pub fn new(image: &ImagePosition, offset: [f32; 2], anchor: SymbolAnchorType) -> Self {
        let size = Vec2::from(image.display_size());
        let top_left = Vec2::from(offset) - size * anchor_alignment(anchor);
        let bounds =
            Bounds { left: top_left.x, top: top_left.y, right: top_left.x + size.x, bottom: top_left.y + size.y };

        let collision_padding = image.content.map_or_else(Padding::default, |content| {
            let pixel_ratio = image.pixel_ratio;
            Padding {
                top: content.top / pixel_ratio,
                right: size.x - content.right / pixel_ratio,
                bottom: size.y - content.bottom / pixel_ratio,
                left: content.left / pixel_ratio,
            }
        });

        PositionedIcon { image: image.clone(), bounds, collision_padding }
    }

    /// Fits the icon to `text` (`icon-text-fit`), drawn at `font_scale`:
    /// stretched to the text's size plus `padding` (top, right, bottom,
    /// left) in the dimensions `fit` names, and centered on the text in the
    /// others, then offset by `offset`. It ignores `icon-anchor`.
    pub fn fit_to_text(
        &mut self,
        text: &Shaping,
        fit: IconTextFitType,
        padding: [f32; 4],
        offset: [f32; 2],
        font_scale: f32,
    ) {
        debug_assert_ne!(fit, IconTextFitType::None);
        debug_assert!(!text.is_empty());
        let [width, height] = self.image.display_size();
        let [top_padding, right_padding, bottom_padding, left_padding] = padding;
        let text_bounds = text.bounds;

        let (text_left, text_right) = (text_bounds.left * font_scale, text_bounds.right * font_scale);
        if matches!(fit, IconTextFitType::Width | IconTextFitType::Both) {
            self.bounds.left = offset[0] + text_left - left_padding;
            self.bounds.right = offset[0] + text_right + right_padding;
        } else {
            self.bounds.left = offset[0] + (text_left + text_right - width) / 2.0;
            self.bounds.right = self.bounds.left + width;
        }

        let (text_top, text_bottom) = (text_bounds.top * font_scale, text_bounds.bottom * font_scale);
        if matches!(fit, IconTextFitType::Height | IconTextFitType::Both) {
            self.bounds.top = offset[1] + text_top - top_padding;
            self.bounds.bottom = offset[1] + text_bottom + bottom_padding;
        } else {
            self.bounds.top = offset[1] + (text_top + text_bottom - height) / 2.0;
            self.bounds.bottom = self.bounds.top + height;
        }
    }

    /// The bounds of an icon fitted to its text, widened or heightened to
    /// keep its content's aspect ratio where the image's `textFitWidth` or
    /// `textFitHeight` asks for it. Other icons keep their bounds.
    #[must_use]
    pub fn text_fit_bounds(&self) -> Bounds {
        let mut bounds = self.bounds;
        let Some(content) = self.image.content else {
            return bounds;
        };
        if self.image.text_fit_width.is_none() && self.image.text_fit_height.is_none() {
            return bounds;
        }
        // The style spec's default for both.
        let fit_width = self.image.text_fit_width.unwrap_or(TextFit::StretchOrShrink);
        let fit_height = self.image.text_fit_height.unwrap_or(TextFit::StretchOrShrink);
        let (width, height) = (bounds.width(), bounds.height());
        let content_aspect_ratio = (content.right - content.left) / (content.bottom - content.top);

        if fit_height == TextFit::Proportional {
            if fit_width == TextFit::Proportional
                || (fit_width == TextFit::StretchOnly && width / height < content_aspect_ratio)
            {
                let new_width = (height * content_aspect_ratio).ceil();
                bounds.left *= new_width / width;
                bounds.right = bounds.left + new_width;
            }
        } else if fit_width == TextFit::Proportional
            && fit_height == TextFit::StretchOnly
            && content_aspect_ratio != 0.0
            && width / height > content_aspect_ratio
        {
            let new_height = (width / content_aspect_ratio).ceil();
            bounds.top *= new_height / height;
            bounds.bottom = bounds.top + new_height;
        }
        // Where neither is proportional, the icon already fits the content.
        bounds
    }
}

/// A line of a label, or the whole label: its characters and their
/// sections.
#[derive(Clone, Copy)]
struct Line<'a> {
    text: &'a [char],
    styles: &'a [SectionIndex],
    sections: &'a [SectionOptions],
}

/// A character of a [`Line`].
struct LineChar<'a> {
    c: char,
    section_index: usize,
    section: &'a SectionOptions,
    /// The glyph that draws it: its glyph PBF glyph, or its glyph of a
    /// section that a font face shaped. `None` for a character beyond the
    /// BMP, or past the glyphs of its shaping.
    glyph: Option<GlyphID>,
    /// Where a font face's shaping places the glyph.
    adjust: Option<GlyphAdjust>,
}

/// `" \t\n\v\f\r"`, which lines don't start or end with.
fn is_trim_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

impl<'a> Line<'a> {
    fn new(text: &'a StyledText, sections: &'a [SectionOptions]) -> Self {
        Line { text: &text.text, styles: &text.styles, sections }
    }

    fn len(&self) -> usize {
        self.text.len()
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn section(&self, index: usize) -> &'a SectionOptions {
        &self.sections[usize::from(self.styles[index])]
    }

    fn slice(self, range: Range<usize>) -> Self {
        Line { text: &self.text[range.clone()], styles: &self.styles[range], sections: self.sections }
    }

    /// The line without leading and trailing whitespace, but for that of
    /// sections drawn from font files.
    fn trimmed(self) -> Self {
        let keep = |i: usize| !is_trim_whitespace(self.text[i]) || self.section(i).type_ != GlyphIDType::FontPBF;
        let Some(start) = (0..self.len()).find(|&i| keep(i)) else {
            return self.slice(0..0);
        };
        let end = (start..self.len()).rfind(|&i| keep(i)).map_or(start, |i| i + 1);
        self.slice(start..end)
    }

    /// The largest scale of a section of the line's characters.
    fn max_scale(&self) -> f32 {
        self.styles.iter().map(|&style| self.sections[usize::from(style)].scale).fold(0.0, f32::max)
    }

    fn chars(self) -> impl Iterator<Item = LineChar<'a>> {
        // A shaped section's glyphs are contiguous, from its first character.
        let mut section_start = 0;
        (0..self.len()).map(move |i| {
            let section_index = usize::from(self.styles[i]);
            if i > 0 && self.styles[i - 1] != self.styles[i] {
                section_start = i;
            }
            let section = &self.sections[section_index];
            let c = self.text[i];
            let (glyph, adjust) = match &section.shaped {
                Some(shaped) => {
                    let offset = i - section_start;
                    (shaped.glyphs.get(offset).copied(), shaped.adjusts.get(offset).copied())
                }
                None => (GlyphID::from_char(c), None),
            };
            LineChar { c, section_index, section, glyph, adjust }
        })
    }
}

/// How far each character of `text` moves the pen: a shaped glyph by its
/// shaping, a glyph PBF glyph or an image by its size, with `spacing`.
pub(crate) fn glyph_advances<'a>(
    text: &'a TaggedString,
    spacing: f32,
    glyphs: &'a GlyphPositions,
    images: &'a ImagePositions,
    text_size: f32,
) -> impl Iterator<Item = f32> + 'a {
    Line::new(text.styled_text(), text.sections()).chars().map(move |c| {
        let section = c.section;
        if section.shaped.is_some() {
            // A glyph without an advance takes no spacing either.
            let advance = c.adjust.map_or(0.0, |adjust| adjust.advance);
            return if advance > 0.01 { advance * section.scale + spacing } else { 0.0 };
        }
        match &section.image_id {
            None => match c.glyph.and_then(|glyph| glyphs.get(&section.font_stack_hash)?.get(&glyph)) {
                Some(Some(glyph)) => glyph.metrics.advance as f32 * section.scale + spacing,
                _ => 0.0,
            },
            Some(image_id) => images
                .get(image_id)
                .map_or(0.0, |image| image.display_size()[0] * section.scale * ONE_EM / text_size + spacing),
        }
    })
}

/// The room a line's content takes across the line: its largest glyph or
/// image.
struct LineContentSize {
    /// The height of a horizontal line.
    horizontal_height: f32,
    /// The width of a vertical line.
    vertical_width: f32,
}

impl LineContentSize {
    fn new(line: Line<'_>, images: &ImagePositions, text_size: f32, max_scale: f32) -> Self {
        let max_glyph_size = max_scale * ONE_EM;
        let image_scale = ONE_EM / text_size;
        let (mut max_width, mut max_height) = (0.0_f32, 0.0_f32);
        for i in 0..line.len() {
            if let Some(image) = line.section(i).image_id.as_ref().and_then(|id| images.get(id)) {
                let [width, height] = image.display_size();
                max_width = max_width.max(width);
                max_height = max_height.max(height);
            }
        }
        LineContentSize {
            horizontal_height: max_glyph_size.max(max_height * image_scale),
            vertical_width: max_glyph_size.max(max_width * image_scale),
        }
    }

    /// How far below the line's top a section `size` tall starts. A vertical
    /// glyph ignores `vertical_align`, as in GL JS.
    fn baseline_offset(&self, size: f32, vertical: bool, vertical_align: VerticalAlign) -> f32 {
        if vertical {
            return self.vertical_width - size;
        }
        let factor = match vertical_align {
            VerticalAlign::Top => 0.0,
            VerticalAlign::Center => 0.5,
            VerticalAlign::Bottom => 1.0,
        };
        (self.horizontal_height - size) * factor
    }
}

const MAX_UPRIGHT_LETTER_RUN: usize = 3;

/// Whether a run of characters that turn with a vertical line stands upright
/// instead: a number, or a short code such as "JR" or "A1". Longer uppercase
/// runs are words.
fn run_is_upright(run: &[char]) -> bool {
    let is_number = run.iter().any(|&c| i18n::is_digit(c))
        && run.iter().all(|&c| i18n::is_digit(c) || i18n::is_punctuation_or_symbol(c));
    let is_short_code =
        run.len() <= MAX_UPRIGHT_LETTER_RUN && run.iter().all(|&c| i18n::is_digit(c) || i18n::is_uppercase(c));
    is_number || is_short_code
}

/// Which characters of `line` stand upright in a vertical line: those that
/// stay upright in vertical writing, the digits and uppercase letters of
/// runs that [`run_is_upright`], and punctuation between upright characters,
/// which takes its vertical form. Keeps words and combining marks together;
/// images and sections drawn from font files delimit runs.
fn upright_characters(line: Line<'_>) -> Vec<bool> {
    let text = line.text;
    let is_text = |i: usize| {
        let section = line.section(i);
        section.type_ == GlyphIDType::FontPBF && section.image_id.is_none()
    };
    let mut upright: Vec<bool> =
        (0..line.len()).map(|i| is_text(i) && i18n::has_upright_vertical_orientation(text[i])).collect();

    let in_run = |upright: &[bool], i: usize| is_text(i) && !upright[i] && !i18n::is_whitespace(text[i]);
    let mut start = 0;
    while start < line.len() {
        if !in_run(&upright, start) {
            start += 1;
            continue;
        }
        let end = (start + 1..line.len()).find(|&i| !in_run(&upright, i)).unwrap_or(line.len());
        if run_is_upright(&text[start..end]) {
            for i in start..end {
                let c = text[i];
                upright[i] =
                    (i18n::is_uppercase(c) || i18n::is_digit(c)) && !i18n::is_char_in_complex_shaping_script(c);
            }
        }
        start = end;
    }

    for i in 0..line.len() {
        if !upright[i]
            && is_text(i)
            && i18n::verticalize_punctuation(text[i]).is_some()
            && (i == 0 || upright[i - 1])
            && upright.get(i + 1).is_none_or(|&next| next)
        {
            upright[i] = true;
        }
    }
    upright
}

/// Moves a line's glyphs left by `justify` of its width.
fn justify_line(glyphs: &mut [PositionedGlyph], justify: f32) {
    if justify == 0.0 {
        return;
    }
    let Some(last) = glyphs.last() else { return };
    let indent = (last.x + last.metrics.advance as f32 * last.scale) * justify;
    for glyph in glyphs {
        glyph.x -= indent;
    }
}

/// Places the glyphs of a label's lines.
struct LineShaper<'a> {
    options: &'a ShapingOptions,
    glyphs: &'a GlyphPositions,
    images: &'a ImagePositions,
}

/// A line's glyphs, placed.
struct ShapedLine {
    glyphs: Vec<PositionedGlyph>,
    /// Where the pen ends up.
    end_x: f32,
    /// How far an inline image reaches below a line of the line's largest
    /// scale.
    image_overflow: f32,
}

impl LineShaper<'_> {
    /// Places the glyphs of `line`, which isn't empty, on a line at `y`.
    fn shape_line(&self, line: Line<'_>, y: f32, max_scale: f32) -> ShapedLine {
        let options = self.options;
        let spacing = options.spacing;
        let content = LineContentSize::new(line, self.images, options.text_size_at_bucket_zoom, max_scale);
        let vertical_text = options.writing_mode == TextWritingModeType::Vertical;
        let upright = (vertical_text && !options.allow_vertical_placement).then(|| upright_characters(line));

        let mut glyphs = Vec::with_capacity(line.len());
        let mut x = 0.0;
        let mut image_overflow = 0.0_f32;
        for (i, c) in line.chars().enumerate() {
            let Some(mut glyph) = c.glyph else { continue };
            let section = c.section;
            let vertical = match &upright {
                Some(upright) => upright[i],
                None => {
                    vertical_text
                        && glyph.type_ == GlyphIDType::FontPBF
                        && if options.allow_vertical_placement {
                            !i18n::is_whitespace(c.c) && !i18n::is_char_in_complex_shaping_script(c.c)
                        } else {
                            i18n::has_upright_vertical_orientation(c.c)
                        }
                }
            };
            // Punctuation that stands upright among upright characters takes
            // its vertical form.
            if upright.is_some()
                && vertical
                && !i18n::has_upright_vertical_orientation(c.c)
                && let Some(form) = i18n::verticalize_punctuation(c.c).and_then(pbf_code)
            {
                glyph = GlyphID::with_type(form, section.type_);
            }

            let (rect, metrics, scale, baseline_offset, advance, vertical_advance, offset);
            match &section.image_id {
                None => {
                    let Some(Some(position)) =
                        self.glyphs.get(&section.font_stack_hash).and_then(|glyphs| glyphs.get(&glyph))
                    else {
                        continue;
                    };
                    rect = position.rect;
                    metrics = position.metrics;
                    scale = section.scale;
                    let mut glyph_offset = Vec2::ZERO;
                    advance = match c.adjust {
                        Some(adjust) => {
                            glyph_offset = Vec2::new(adjust.x_offset, adjust.y_offset) * scale;
                            adjust.advance
                        }
                        None => metrics.advance as f32,
                    };
                    // A glyph without an advance, such as a combining mark,
                    // aligns to the glyph before it, without its spacing.
                    if advance < 0.01 {
                        glyph_offset.x -= spacing;
                    }
                    offset = glyph_offset;
                    baseline_offset = content.baseline_offset(scale * ONE_EM, vertical, section.vertical_align);
                    vertical_advance = ONE_EM;
                }
                Some(image_id) => {
                    let Some(image) = self.images.get(image_id) else {
                        continue;
                    };
                    let [width, height] = image.display_size();
                    // An image narrower than its padding has no size.
                    let (pixel_width, pixel_height) = (width as u32, height as u32);
                    metrics = GlyphMetrics {
                        width: pixel_width,
                        height: pixel_height,
                        left: f32::from(ImagePosition::PADDING),
                        top: -f32::from(Glyph::BORDER_SIZE),
                        advance: if vertical { pixel_height } else { pixel_width },
                        is_double_resolution: false,
                    };
                    rect = image.padded_rect;
                    scale = section.scale * ONE_EM / options.text_size_at_bucket_zoom;
                    baseline_offset = content.baseline_offset(height * scale, vertical, section.vertical_align);
                    advance = metrics.advance as f32;
                    vertical_advance = advance;
                    offset = Vec2::ZERO;
                    // An image taller than an em at the line's largest scale
                    // pushes the line down.
                    image_overflow =
                        image_overflow.max((if vertical { width } else { height }) * scale - ONE_EM * max_scale);
                }
            }

            let position = if vertical { Vec2::ZERO } else { offset } + Vec2::new(x, y + baseline_offset);
            glyphs.push(PositionedGlyph {
                glyph,
                x: position.x,
                y: position.y,
                vertical,
                scale,
                rect,
                metrics,
                image_id: section.image_id.clone(),
                section_index: c.section_index,
            });
            if vertical {
                x += vertical_advance * scale + spacing;
            } else if advance > 0.01 {
                x += advance * scale + spacing;
            }
        }
        ShapedLine { glyphs, end_x: x, image_overflow }
    }
}

/// Places the glyphs of `lines` and aligns them on the anchor.
fn shape_lines(
    lines: &[StyledText],
    sections: &[SectionOptions],
    options: &ShapingOptions,
    glyphs: &GlyphPositions,
    images: &ImagePositions,
) -> Shaping {
    let shaper = LineShaper { options, glyphs, images };
    let justify = justify_factor(options.justify);
    let mut positioned_lines = Vec::with_capacity(lines.len());
    let mut y = Shaping::Y_OFFSET;
    let mut max_line_length = 0.0_f32;
    let mut max_line_height = 0.0_f32;

    for line in lines {
        // Whitespace at a line's ends would throw off its justification.
        let line = Line::new(line, sections).trimmed();
        if line.is_empty() {
            positioned_lines.push(PositionedLine::default());
            y += options.line_height;
            continue;
        }

        let max_scale = line.max_scale();
        let ShapedLine { glyphs: mut line_glyphs, end_x, image_overflow } = shaper.shape_line(line, y, max_scale);
        if !line_glyphs.is_empty() {
            // Not counting the spacing after the last glyph.
            max_line_length = max_line_length.max(end_x - options.spacing);
            justify_line(&mut line_glyphs, justify);
        }

        let line_height = options.line_height * max_scale + image_overflow;
        y += line_height;
        max_line_height = max_line_height.max(line_height);
        let line_offset = image_overflow.max((max_scale - 1.0) * ONE_EM);
        positioned_lines.push(PositionedLine { glyphs: line_glyphs, line_offset });
    }

    let alignment = anchor_alignment(options.anchor);
    let height = y - Shaping::Y_OFFSET;
    let shift_y = if max_line_height == options.line_height {
        (0.5 - alignment.y * lines.len() as f32) * options.line_height
    } else {
        -height * alignment.y - Shaping::Y_OFFSET
    };
    let shift = Vec2::new((justify - alignment.x) * max_line_length, shift_y);
    let mut verticalizable = false;
    let mut icons_in_text = false;
    for glyph in positioned_lines.iter_mut().flat_map(|line| &mut line.glyphs) {
        glyph.x += shift.x;
        glyph.y += shift.y;
        verticalizable |= glyph.vertical;
        icons_in_text |= glyph.image_id.is_some();
    }

    let [x, y] = options.offset;
    let left = x - alignment.x * max_line_length;
    let top = y - alignment.y * height;
    Shaping {
        lines: positioned_lines,
        bounds: Bounds { left, top, right: left + max_line_length, bottom: top + height },
        verticalizable,
        icons_in_text,
    }
}

/// The lines of text that holds shaped sections, broken at `line_breaks`
/// and the ends of paragraphs, in visual order. The bidi analysis covers the
/// whole text, and sees each shaped section as characters of the section's
/// direction, which it moves as a block without reversing them again.
fn reorder_shaped_lines(text: &StyledText, sections: &[SectionOptions], line_breaks: VecSet<usize>) -> Vec<StyledText> {
    let bidi_text: Vec<char> = text
        .text
        .iter()
        .zip(&text.styles)
        .map(|(&c, &style)| {
            let section = &sections[usize::from(style)];
            match (section.type_, section.rtl) {
                (GlyphIDType::FontPBF, _) => c,
                (_, true) => '\u{05D0}',
                (_, false) => 'a',
            }
        })
        .collect();
    let block_styles: Vec<bool> = sections.iter().map(|section| section.type_ != GlyphIDType::FontPBF).collect();
    bidi::reorder_styled_lines_with_blocks(text, line_breaks, &bidi_text, &block_styles)
}

/// `text` broken into lines, each in visual order.
fn break_lines(
    text: &TaggedString,
    options: &ShapingOptions,
    glyphs: &GlyphPositions,
    images: &ImagePositions,
) -> Vec<StyledText> {
    let styled = text.styled_text();
    if styled.text.is_empty() {
        return Vec::new();
    }
    let line_breaks =
        || determine_line_breaks(text, options.spacing, options.max_width, glyphs, images, options.text_size);
    match text.sections() {
        // Text that one font face shaped stays on one line.
        [section] if section.type_ != GlyphIDType::FontPBF => vec![styled.clone()],
        [_] => bidi::reorder_lines(&styled.text, line_breaks())
            .into_iter()
            .map(|text| StyledText { styles: vec![0; text.len()], text })
            .collect(),
        sections if text.has_font_sections() => reorder_shaped_lines(styled, sections, line_breaks()),
        _ => bidi::reorder_styled_lines(styled, line_breaks()),
    }
}

/// Shapes `text` with `options`, from the glyphs and images it draws.
#[must_use]
pub fn shape_text(
    text: &TaggedString,
    options: &ShapingOptions,
    glyphs: &GlyphPositions,
    images: &ImagePositions,
) -> Shaping {
    debug_assert!(options.text_size != 0.0);
    let lines = break_lines(text, options, glyphs, images);
    shape_lines(&lines, text.sections(), options, glyphs, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trimmed(text: &TaggedString) -> String {
        Line::new(text.styled_text(), text.sections()).trimmed().text.iter().collect()
    }

    fn u(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    // gtest: TaggedString.Trim
    #[test]
    fn trims_lines() {
        let options = || SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None);
        let basic = TaggedString::new(u(" \t\ntrim that and not this  \n\t"), options());
        assert_eq!(trimmed(&basic), "trim that and not this");

        let mut two_sections = TaggedString::default();
        two_sections.add_text_section(&u(" \t\ntrim that"), SectionOptions { scale: 1.5, ..options() });
        two_sections.add_text_section(&u(" and not this  \n\t"), SectionOptions { scale: 0.5, ..options() });
        assert_eq!(trimmed(&two_sections), "trim that and not this");

        let empty = TaggedString::new(u("\n\t\u{b} \r  \t\n"), options());
        assert_eq!(trimmed(&empty), "");

        let no_trim = TaggedString::new(u("no trim!"), options());
        assert_eq!(trimmed(&no_trim), "no trim!");
    }

    #[test]
    fn keeps_whitespace_of_font_sections() {
        let face = crate::text::glyph::face_type("shaping-trim-test", "font", &[(0, 0xFFFF)]);
        let mut text = TaggedString::default();
        text.add_text_section(&u("  a"), SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None));
        text.add_text_section(&u(" b "), SectionOptions::new(1.0, vec![], face, None));
        text.add_text_section(&u("  "), SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None));
        assert_eq!(trimmed(&text), "a b ");
    }

    #[test]
    fn anchor_alignment_and_justification_agree() {
        use SymbolAnchorType as A;
        for anchor in
            [A::Center, A::Left, A::Right, A::Top, A::Bottom, A::TopLeft, A::TopRight, A::BottomLeft, A::BottomRight]
        {
            assert_eq!(anchor_alignment(anchor).x, justify_factor(anchor_justification(anchor)));
        }
        assert_eq!(anchor_alignment(A::BottomLeft), Vec2::new(0.0, 1.0));
    }
}
