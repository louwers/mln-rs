// Ports src/mln/text/quads.hpp and src/mln/text/quads.cpp.

use geometry::Point;
use mln_math::{deg2radf, max_f32, min_f32};
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style_types::image::{ImageMap, ImagePosition, ImageStretches, RectU16};
use mln_style_types::{AlignmentType, SymbolPlacementType};
use mln_text::PositionedIcon;
use mln_text::glyph::{Shaping, WritingModeType};
use mln_util::constants::ONE_EM;

use crate::layout::symbol_instance::SymbolContent;

/// `SymbolQuad`.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolQuad {
    pub tl: Point<f32>,
    pub tr: Point<f32>,
    pub bl: Point<f32>,
    pub br: Point<f32>,
    pub tex: RectU16,
    pub pixel_offset_tl: Point<f32>,
    pub pixel_offset_br: Point<f32>,
    pub glyph_offset: Point<f32>,
    pub writing_mode: WritingModeType,
    pub is_sdf: bool,
    pub section_index: usize,
    pub min_font_scale: Point<f32>,
}

impl SymbolQuad {
    /// `SymbolQuad(tl, tr, bl, br, tex, writingMode, glyphOffset, isSDF,
    /// pixelOffsetTL, pixelOffsetBR, minFontScale, sectionIndex = 0)`.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        tl: Point<f32>,
        tr: Point<f32>,
        bl: Point<f32>,
        br: Point<f32>,
        tex: RectU16,
        writing_mode: WritingModeType,
        glyph_offset: Point<f32>,
        is_sdf: bool,
        pixel_offset_tl: Point<f32>,
        pixel_offset_br: Point<f32>,
        min_font_scale: Point<f32>,
        section_index: usize,
    ) -> Self {
        SymbolQuad {
            tl,
            tr,
            bl,
            br,
            tex,
            pixel_offset_tl,
            pixel_offset_br,
            glyph_offset,
            writing_mode,
            is_sdf,
            section_index,
            min_font_scale,
        }
    }
}

/// `SymbolQuads`.
pub type SymbolQuads = Vec<SymbolQuad>;

/// `border = ImagePosition::padding` (a `uint16_t`).
const BORDER: u16 = ImagePosition::PADDING;

/// `Point<float> - Point<float>`.
fn sub(a: Point<f32>, b: Point<f32>) -> Point<f32> {
    Point::new(a.x - b.x, a.y - b.y)
}

/// `Point<float> + Point<float>`.
fn add(a: Point<f32>, b: Point<f32>) -> Point<f32> {
    Point::new(a.x + b.x, a.y + b.y)
}

/// `util::rotate(Point<float>, float angle)` (util/math.hpp).
fn rotate(a: Point<f32>, angle: f32) -> Point<f32> {
    let cos = angle.cos();
    let sin = angle.sin();
    let x = cos * a.x - sin * a.y;
    let y = sin * a.x + cos * a.y;
    Point::new(x, y)
}

/// `util::matrixMultiply(std::array<float, 4>, Point<float>)` (util/math.hpp).
fn matrix_multiply(m: &[f32; 4], p: Point<f32>) -> Point<f32> {
    Point::new(m[0] * p.x + m[1] * p.y, m[2] * p.x + m[3] * p.y)
}

/// `computeStretchSum`.
#[must_use]
pub fn compute_stretch_sum(stretches: &ImageStretches) -> f32 {
    let mut sum: f32 = 0.0;
    for stretch in stretches {
        sum += stretch.1 - stretch.0;
    }
    sum
}

/// `sumWithinRange`.
#[must_use]
pub fn sum_within_range(stretches: &ImageStretches, min: f32, max: f32) -> f32 {
    let mut sum: f32 = 0.0;
    for stretch in stretches {
        sum += max_f32(min, min_f32(max, stretch.1)) - max_f32(min, min_f32(max, stretch.0));
    }
    sum
}

/// `getEmOffset`.
#[inline]
#[must_use]
pub fn get_em_offset(stretch_offset: f32, stretch_size: f32, icon_size: f32, icon_offset: f32) -> f32 {
    icon_offset + icon_size * stretch_offset / stretch_size
}

/// `getPxOffset`.
#[inline]
#[must_use]
pub fn get_px_offset(fixed_offset: f32, fixed_size: f32, stretch_offset: f32, stretch_size: f32) -> f32 {
    fixed_offset - fixed_size * stretch_offset / stretch_size
}

/// `Cut`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub fixed: f32,
    pub stretch: f32,
}

/// `Cuts`.
pub type Cuts = Vec<Cut>;

/// `stretchZonesToCuts`.
#[must_use]
pub fn stretch_zones_to_cuts(stretch_zones: &ImageStretches, fixed_size: f32, stretch_size: f32) -> Cuts {
    // `-border` is the `int` -1.
    let mut cuts: Cuts = vec![Cut { fixed: -i32::from(BORDER) as f32, stretch: 0.0 }];

    for zone in stretch_zones {
        let c1 = zone.0;
        let c2 = zone.1;
        let last_stretch = cuts.last().expect("cuts is never empty").stretch;
        cuts.push(Cut { fixed: c1 - last_stretch, stretch: last_stretch });
        cuts.push(Cut { fixed: c1 - last_stretch, stretch: last_stretch + (c2 - c1) });
    }
    cuts.push(Cut { fixed: fixed_size + f32::from(BORDER), stretch: stretch_size });
    cuts
}

/// `getIconQuads`.
#[must_use]
pub fn get_icon_quads(
    shaped_icon: &PositionedIcon,
    icon_rotate: f32,
    icon_type: SymbolContent,
    has_icon_text_fit: bool,
) -> SymbolQuads {
    let mut quads = SymbolQuads::new();

    let image: &ImagePosition = shaped_icon.image();
    let pixel_ratio = image.pixel_ratio;
    // `int` arithmetic narrowed to `uint16_t`.
    let image_width = (i32::from(image.padded_rect.w) - 2 * i32::from(BORDER)) as u16;
    let image_height = (i32::from(image.padded_rect.h) - 2 * i32::from(BORDER)) as u16;

    let stretch_x_full: ImageStretches = vec![(0.0, f32::from(image_width))];
    let stretch_y_full: ImageStretches = vec![(0.0, f32::from(image_height))];
    let stretch_x: &ImageStretches = if image.stretch_x.is_empty() { &stretch_x_full } else { &image.stretch_x };
    let stretch_y: &ImageStretches = if image.stretch_y.is_empty() { &stretch_y_full } else { &image.stretch_y };

    let stretch_width = compute_stretch_sum(stretch_x);
    let stretch_height = compute_stretch_sum(stretch_y);
    let fixed_width = f32::from(image_width) - stretch_width;
    let fixed_height = f32::from(image_height) - stretch_height;

    let mut stretch_offset_x: f32 = 0.0;
    let mut stretch_content_width = stretch_width;
    let mut stretch_offset_y: f32 = 0.0;
    let mut stretch_content_height = stretch_height;
    let mut fixed_offset_x: f32 = 0.0;
    let mut fixed_content_width = fixed_width;
    let mut fixed_offset_y: f32 = 0.0;
    let mut fixed_content_height = fixed_height;

    let mut icon = shaped_icon.clone();

    if has_icon_text_fit && let Some(content) = &image.content {
        let content_width = content.right - content.left;
        let content_height = content.bottom - content.top;
        if image.text_fit_width.is_some() || image.text_fit_height.is_some() {
            icon = icon.apply_text_fit();
        }
        stretch_offset_x = sum_within_range(stretch_x, 0.0, content.left);
        stretch_offset_y = sum_within_range(stretch_y, 0.0, content.top);
        stretch_content_width = sum_within_range(stretch_x, content.left, content.right);
        stretch_content_height = sum_within_range(stretch_y, content.top, content.bottom);
        fixed_offset_x = content.left - stretch_offset_x;
        fixed_offset_y = content.top - stretch_offset_y;
        fixed_content_width = content_width - stretch_content_width;
        fixed_content_height = content_height - stretch_content_height;
    }

    let mut matrix: Option<[f32; 4]> = None;
    if icon_rotate != 0.0 {
        let angle = deg2radf(icon_rotate);
        let angle_sin = angle.sin();
        let angle_cos = angle.cos();
        matrix = Some([angle_cos, -angle_sin, angle_sin, angle_cos]);
    }

    let icon_left = icon.left();
    let icon_top = icon.top();
    let icon_width = icon.right() - icon_left;
    let icon_height = icon.bottom() - icon_top;

    let mut make_box = |left: Cut, top: Cut, right: Cut, bottom: Cut| {
        let left_em = get_em_offset(left.stretch - stretch_offset_x, stretch_content_width, icon_width, icon_left);
        let left_px = get_px_offset(left.fixed - fixed_offset_x, fixed_content_width, left.stretch, stretch_width);

        let top_em = get_em_offset(top.stretch - stretch_offset_y, stretch_content_height, icon_height, icon_top);
        let top_px = get_px_offset(top.fixed - fixed_offset_y, fixed_content_height, top.stretch, stretch_height);

        let right_em = get_em_offset(right.stretch - stretch_offset_x, stretch_content_width, icon_width, icon_left);
        let right_px = get_px_offset(right.fixed - fixed_offset_x, fixed_content_width, right.stretch, stretch_width);

        let bottom_em = get_em_offset(bottom.stretch - stretch_offset_y, stretch_content_height, icon_height, icon_top);
        let bottom_px =
            get_px_offset(bottom.fixed - fixed_offset_y, fixed_content_height, bottom.stretch, stretch_height);

        let mut tl = Point::new(left_em, top_em);
        let mut tr = Point::new(right_em, top_em);
        let mut br = Point::new(right_em, bottom_em);
        let mut bl = Point::new(left_em, bottom_em);
        let pixel_offset_tl = Point::new(left_px / pixel_ratio, top_px / pixel_ratio);
        let pixel_offset_br = Point::new(right_px / pixel_ratio, bottom_px / pixel_ratio);

        if let Some(matrix) = &matrix {
            tl = matrix_multiply(matrix, tl);
            tr = matrix_multiply(matrix, tr);
            bl = matrix_multiply(matrix, bl);
            br = matrix_multiply(matrix, br);
        }

        let x1 = left.stretch + left.fixed;
        let x2 = right.stretch + right.fixed;
        let y1 = top.stretch + top.fixed;
        let y2 = bottom.stretch + bottom.fixed;

        // TODO: consider making texture coordinates float instead of uint16_t
        // (`paddedRect.x + border` is an `int`; adding the float makes it a
        // float, then `static_cast<uint16_t>`.)
        let sub_rect = RectU16 {
            x: ((i32::from(image.padded_rect.x) + i32::from(BORDER)) as f32 + x1) as u16,
            y: ((i32::from(image.padded_rect.y) + i32::from(BORDER)) as f32 + y1) as u16,
            w: (x2 - x1) as u16,
            h: (y2 - y1) as u16,
        };

        let min_font_scale_x = fixed_content_width / pixel_ratio / icon_width;
        let min_font_scale_y = fixed_content_height / pixel_ratio / icon_height;

        // Icon quad is padded, so texture coordinates also need to be padded.
        quads.push(SymbolQuad::new(
            tl,
            tr,
            bl,
            br,
            sub_rect,
            WritingModeType::None,
            Point::new(0.0, 0.0),
            icon_type == SymbolContent::IconSDF,
            pixel_offset_tl,
            pixel_offset_br,
            Point::new(min_font_scale_x, min_font_scale_y),
            0,
        ));
    };

    if !has_icon_text_fit || (image.stretch_x.is_empty() && image.stretch_y.is_empty()) {
        make_box(
            Cut { fixed: 0.0, stretch: -1.0 },
            Cut { fixed: 0.0, stretch: -1.0 },
            Cut { fixed: 0.0, stretch: (i32::from(image_width) + 1) as f32 },
            Cut { fixed: 0.0, stretch: (i32::from(image_height) + 1) as f32 },
        );
    } else {
        let x_cuts = stretch_zones_to_cuts(stretch_x, fixed_width, stretch_width);
        let y_cuts = stretch_zones_to_cuts(stretch_y, fixed_height, stretch_height);

        for xi in 0..x_cuts.len() - 1 {
            let x1 = x_cuts[xi];
            let x2 = x_cuts[xi + 1];
            for yi in 0..y_cuts.len() - 1 {
                let y1 = y_cuts[yi];
                let y2 = y_cuts[yi + 1];
                make_box(x1, y1, x2, y2);
            }
        }
    }

    quads
}

/// `getGlyphQuads`.
#[must_use]
pub fn get_glyph_quads(
    shaped_text: &Shaping,
    text_offset: [f32; 2],
    layout: &SymbolLayoutProperties::Evaluated,
    placement: SymbolPlacementType,
    image_map: &ImageMap,
    allow_vertical_placement: bool,
) -> SymbolQuads {
    let text_rotate = deg2radf(layout.text_rotate);
    let along_line = layout.text_rotation_alignment == AlignmentType::Map && placement != SymbolPlacementType::Point;

    let mut quads = SymbolQuads::new();

    for line in &shaped_text.positioned_lines {
        for positioned_glyph in &line.positioned_glyphs {
            // `rect.hasArea()`.
            if !(positioned_glyph.rect.w != 0 && positioned_glyph.rect.h != 0) {
                continue;
            }

            // The rects have an additional buffer that is not included in their size;
            let glyph_padding: f32 = 1.0;
            let mut rect_buffer: f32 = 3.0 + glyph_padding;
            let mut pixel_ratio: f32 = 1.0;
            let mut line_offset: f32 = 0.0;
            let rotate_vertical_glyph = (along_line || allow_vertical_placement) && positioned_glyph.vertical;
            let half_advance = positioned_glyph.metrics.advance as f32 * positioned_glyph.scale / 2.0;
            let rect = &positioned_glyph.rect;
            let mut is_sdf = true;

            // Align images and scaled glyphs in the middle of a vertical line.
            if allow_vertical_placement && shaped_text.verticalizable {
                let scaled_glyph_offset = (positioned_glyph.scale - 1.0) * ONE_EM;
                let image_offset = (ONE_EM - positioned_glyph.metrics.width as f32 * positioned_glyph.scale) / 2.0;
                line_offset = line.line_offset / 2.0
                    - (if positioned_glyph.image_id.is_some() { -image_offset } else { scaled_glyph_offset });
            }

            if let Some(image_id) = &positioned_glyph.image_id {
                let Some(image) = image_map.get(image_id) else {
                    continue;
                };
                pixel_ratio = image.pixel_ratio;
                rect_buffer = f32::from(ImagePosition::PADDING) / pixel_ratio;
                is_sdf = image.sdf;
            }

            let glyph_offset = if along_line {
                Point::new(positioned_glyph.x + half_advance, positioned_glyph.y)
            } else {
                Point::new(0.0, 0.0)
            };

            let mut built_in_offset = if along_line {
                Point::new(0.0, 0.0)
            } else {
                Point::new(
                    positioned_glyph.x + half_advance + text_offset[0],
                    positioned_glyph.y + text_offset[1] - line_offset,
                )
            };

            let mut verticalized_label_offset = Point::new(0.0f32, 0.0f32);
            if rotate_vertical_glyph {
                // Vertical POI labels, that are rotated 90deg CW and whose
                // glyphs must preserve upright orientation need to be rotated
                // 90deg CCW. After quad is rotated, it is translated to the
                // original built-in offset.
                verticalized_label_offset = built_in_offset;
                built_in_offset = Point::new(0.0, 0.0);
            }

            // rectBuffer is in 1x logical units; only rect.w/h carry the texture
            // scale. GL JS keeps 2x glyphs centered by storing fractional
            // left/top metrics, so native local glyphs do the same.
            let texture_scale: f32 = if positioned_glyph.metrics.is_double_resolution { 2.0 } else { 1.0 };
            let x1 = (positioned_glyph.metrics.left - rect_buffer) * positioned_glyph.scale - half_advance
                + built_in_offset.x;
            let y1 = (-positioned_glyph.metrics.top - rect_buffer) * positioned_glyph.scale + built_in_offset.y;
            let x2 = x1 + f32::from(rect.w) / texture_scale * positioned_glyph.scale / pixel_ratio;
            let y2 = y1 + f32::from(rect.h) / texture_scale * positioned_glyph.scale / pixel_ratio;

            let mut tl = Point::new(x1, y1);
            let mut tr = Point::new(x2, y1);
            let mut bl = Point::new(x1, y2);
            let mut br = Point::new(x2, y2);

            if rotate_vertical_glyph {
                // Vertical-supporting glyphs are laid out in 24x24 point boxes
                // (1 square em) In horizontal orientation, the y values for
                // glyphs are below the midline and we use a "yOffset" of -17 to
                // pull them up to the middle. By rotating counter-clockwise
                // around the point at the center of the left edge of a 24x24
                // layout box centered below the midline, we align the center of
                // the glyphs with the horizontal midline, so the yOffset is no
                // longer necessary, but we also pull the glyph to the left
                // along the x axis. The y coordinate includes baseline yOffset,
                // therefore, needs to be accounted for when glyph is rotated
                // and translated.

                let center = Point::new(-half_advance, half_advance - Shaping::Y_OFFSET as f32);
                let vertical_rotation = -std::f32::consts::PI / 2.0;

                // xHalfWidhtOffsetcorrection is a difference between full-width
                // and half-width advance, should be 0 for full-width glyphs and
                // will pull up half-width glyphs.
                let x_half_widht_offsetcorrection = ONE_EM / 2.0 - half_advance;
                let y_image_offset_correction =
                    if positioned_glyph.image_id.is_some() { x_half_widht_offsetcorrection } else { 0.0 };
                let x_offset_correction = Point::new(
                    5.0 - Shaping::Y_OFFSET as f32 - x_half_widht_offsetcorrection,
                    -y_image_offset_correction,
                );

                let transform = |p: Point<f32>| {
                    add(
                        add(add(rotate(sub(p, center), vertical_rotation), center), x_offset_correction),
                        verticalized_label_offset,
                    )
                };
                tl = transform(tl);
                tr = transform(tr);
                bl = transform(bl);
                br = transform(br);
            }

            if text_rotate != 0.0 {
                // Compute the transformation matrix.
                let angle_sin = text_rotate.sin();
                let angle_cos = text_rotate.cos();
                let matrix: [f32; 4] = [angle_cos, -angle_sin, angle_sin, angle_cos];

                tl = matrix_multiply(&matrix, tl);
                tr = matrix_multiply(&matrix, tr);
                bl = matrix_multiply(&matrix, bl);
                br = matrix_multiply(&matrix, br);
            }

            let pixel_offset_tl = Point::new(0.0f32, 0.0f32);
            let pixel_offset_br = Point::new(0.0f32, 0.0f32);
            let min_font_scale = Point::new(0.0f32, 0.0f32);

            quads.push(SymbolQuad::new(
                tl,
                tr,
                bl,
                br,
                *rect,
                shaped_text.writing_mode,
                glyph_offset,
                is_sdf,
                pixel_offset_tl,
                pixel_offset_br,
                min_font_scale,
                positioned_glyph.section_index,
            ));
        }
    }

    quads
}

#[cfg(test)]
mod tests {
    // Ports test/text/quads.test.cpp.
    use mln_style_types::image::Impl as ImageImpl;
    use mln_style_types::{IconTextFitType, SymbolAnchorType};
    use mln_text::glyph::{GlyphID, GlyphMetrics, PositionedGlyph, PositionedLine};
    use mln_tile_id::geometry_tile_data::GeometryCoordinates;
    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;

    use super::*;
    use crate::geometry::anchor::Anchor;

    /// gtest `EXPECT_FLOAT_EQ`: within 4 ULPs.
    fn assert_float_eq(actual: f32, expected: f32) {
        fn biased(f: f32) -> u32 {
            let bits = f.to_bits();
            if bits & 0x8000_0000 != 0 { !bits + 1 } else { bits | 0x8000_0000 }
        }
        let (a, b) = (biased(actual), biased(expected));
        let distance = a.abs_diff(b);
        assert!(distance <= 4, "expected {expected}, got {actual}");
    }

    fn test_image_impl() -> ImageImpl {
        ImageImpl::new(
            "test".to_owned(),
            PremultipliedImage::new(Size { width: 1, height: 1 }),
            1.0,
            false,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .expect("valid image")
    }

    fn rect(x: u16, y: u16, w: u16, h: u16) -> RectU16 {
        RectU16 { x, y, w, h }
    }

    fn positioned_glyph(code: u32, rect: RectU16, metrics: GlyphMetrics) -> PositionedGlyph {
        PositionedGlyph::new(GlyphID::new(code), 0.0, 0.0, false, 0, 1.0, rect, metrics, None, 0)
    }

    // Atlas padding is not scaled with textureScale, so a 2x glyph quad is 1
    // logical pixel smaller than its 1x equivalent. GL JS-style fractional metrics
    // keep its center aligned with the 1x quad.
    // gtest: getGlyphQuads.DoubleResolutionGlyphPreservesLogicalQuadSize
    #[test]
    fn get_glyph_quads_double_resolution_glyph_preserves_logical_quad_size() {
        let metrics1x =
            GlyphMetrics { width: 24, height: 24, left: 0.0, top: -8.0, advance: 24, is_double_resolution: false };

        let mut metrics2x = metrics1x;
        metrics2x.left += 0.5;
        metrics2x.top -= 0.5;
        metrics2x.is_double_resolution = true;

        // Rects reflect the real DynamicTextureAtlas output: bitmap.size + 2 *
        // ImagePosition::padding (= 2). 1x bitmap 30 -> rect 32; 2x bitmap 60 -> rect 62.
        let mut shaped_text1x = Shaping::default();
        shaped_text1x.positioned_lines.push(PositionedLine::default());
        shaped_text1x.positioned_lines.last_mut().unwrap().positioned_glyphs.push(positioned_glyph(
            'A' as u32,
            rect(0, 0, 32, 32),
            metrics1x,
        ));

        let mut shaped_text2x = Shaping::default();
        shaped_text2x.positioned_lines.push(PositionedLine::default());
        shaped_text2x.positioned_lines.last_mut().unwrap().positioned_glyphs.push(positioned_glyph(
            'A' as u32,
            rect(0, 0, 62, 62),
            metrics2x,
        ));

        let layout = SymbolLayoutProperties::Evaluated::default();
        let image_map = ImageMap::new();
        let quads1x =
            get_glyph_quads(&shaped_text1x, [0.0, 0.0], &layout, SymbolPlacementType::Point, &image_map, false);
        let quads2x =
            get_glyph_quads(&shaped_text2x, [0.0, 0.0], &layout, SymbolPlacementType::Point, &image_map, false);

        assert_eq!(quads1x.len(), 1);
        assert_eq!(quads2x.len(), 1);

        // The 2x quad is 1 logical pixel smaller, and shifted by 0.5px to keep the
        // same center as the 1x quad.
        assert_float_eq(quads1x[0].tl.x + 0.5, quads2x[0].tl.x);
        assert_float_eq(quads1x[0].tl.y + 0.5, quads2x[0].tl.y);
        assert_float_eq(quads1x[0].br.x - 0.5, quads2x[0].br.x);
        assert_float_eq(quads1x[0].br.y - 0.5, quads2x[0].br.y);

        assert_float_eq(quads1x[0].br.x - quads1x[0].tl.x, 32.0);
        assert_float_eq(quads2x[0].br.x - quads2x[0].tl.x, 31.0);
        assert_float_eq(quads1x[0].br.y - quads1x[0].tl.y, 32.0);
        assert_float_eq(quads2x[0].br.y - quads2x[0].tl.y, 31.0);
    }

    // gtest: getIconQuads.normal
    #[test]
    fn get_icon_quads_normal() {
        let _layout = SymbolLayoutProperties::Evaluated::default();
        let _anchor = Anchor::new(2.0, 3.0, 0.0, Some(0));
        let image = ImagePosition::new(rect(0, 0, 15, 11), &test_image_impl(), 0);

        let shaped_icon = PositionedIcon::shape_icon(&image, &[-6.5, -4.5], SymbolAnchorType::Center);

        let _line = GeometryCoordinates::new();

        let quads = get_icon_quads(&shaped_icon, 0.0, SymbolContent::IconRGBA, false);

        assert_eq!(quads.len(), 1);
        let quad = &quads[0];
        assert_float_eq(quad.tl.x, -14.0);
        assert_float_eq(quad.tl.y, -10.0);
        assert_float_eq(quad.tr.x, 1.0);
        assert_float_eq(quad.tr.y, -10.0);
        assert_float_eq(quad.bl.x, -14.0);
        assert_float_eq(quad.bl.y, 1.0);
        assert_float_eq(quad.br.x, 1.0);
        assert_float_eq(quad.br.y, 1.0);
    }

    fn assert_quad(quad: &SymbolQuad, left: f32, top: f32, right: f32, bottom: f32) {
        assert_float_eq(quad.tl.x, left);
        assert_float_eq(quad.tl.y, top);
        assert_float_eq(quad.tr.x, right);
        assert_float_eq(quad.tr.y, top);
        assert_float_eq(quad.bl.x, left);
        assert_float_eq(quad.bl.y, bottom);
        assert_float_eq(quad.br.x, right);
        assert_float_eq(quad.br.y, bottom);
    }

    // gtest: getIconQuads.style
    #[test]
    fn get_icon_quads_style() {
        let _anchor = Anchor::new(0.0, 0.0, 0.0, Some(0));
        let image = ImagePosition::new(rect(0, 0, 20, 20), &test_image_impl(), 0);

        let _line = GeometryCoordinates::new();
        let mut shaped_text = Shaping { top: -10.0, bottom: 30.0, left: -60.0, right: 20.0, ..Default::default() };
        shaped_text.positioned_lines.push(PositionedLine::default());
        shaped_text.positioned_lines.last_mut().unwrap().positioned_glyphs.push(positioned_glyph(
            32,
            RectU16::default(),
            GlyphMetrics::default(),
        ));

        let fit = |text_fit: IconTextFitType, padding: [f32; 4], font_scale: f32| {
            let mut shaped_icon = PositionedIcon::shape_icon(&image, &[-9.5, -9.5], SymbolAnchorType::Center);
            shaped_icon.fit_icon_to_text(&shaped_text, text_fit, &padding, &[0.0, 0.0], font_scale);
            let quads = get_icon_quads(&shaped_icon, 0.0, SymbolContent::IconRGBA, false);
            assert_eq!(quads.len(), 1);
            quads[0].clone()
        };

        // none
        {
            let shaped_icon = PositionedIcon::shape_icon(&image, &[-9.5, -9.5], SymbolAnchorType::Center);

            assert_float_eq(-18.5, shaped_icon.top());
            assert_float_eq(-0.5, shaped_icon.right());
            assert_float_eq(-0.5, shaped_icon.bottom());
            assert_float_eq(-18.5, shaped_icon.left());

            let quads = get_icon_quads(&shaped_icon, 0.0, SymbolContent::IconRGBA, false);

            assert_eq!(quads.len(), 1);
            assert_quad(&quads[0], -19.5, -19.5, 0.5, 0.5);
        }

        // width
        assert_quad(&fit(IconTextFitType::Width, [0.0, 0.0, 0.0, 0.0], 24.0 / 24.0), -64.444_44, 0.0, 24.444_443, 20.0);

        // width x textSize
        assert_quad(
            &fit(IconTextFitType::Width, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -32.222_22,
            -5.0,
            12.222_221,
            15.0,
        );

        // width x textSize + padding
        assert_quad(
            &fit(IconTextFitType::Width, [5.0, 10.0, 5.0, 10.0], 12.0 / 24.0),
            -43.333_332,
            -5.0,
            23.333_332,
            15.0,
        );

        // height
        assert_quad(
            &fit(IconTextFitType::Height, [0.0, 0.0, 0.0, 0.0], 24.0 / 24.0),
            -30.0,
            -12.222_221,
            -10.0,
            32.222_22,
        );

        // height x textSize
        assert_quad(
            &fit(IconTextFitType::Height, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -20.0,
            -6.111_110_7,
            0.0,
            16.111_11,
        );

        // height x textSize + padding
        assert_quad(
            &fit(IconTextFitType::Height, [5.0, 10.0, 5.0, 20.0], 12.0 / 24.0),
            -20.0,
            -11.666_666,
            0.0,
            21.666_666,
        );

        // both
        assert_quad(
            &fit(IconTextFitType::Both, [0.0, 0.0, 0.0, 0.0], 24.0 / 24.0),
            -64.444_44,
            -12.222_221,
            24.444_443,
            32.222_22,
        );

        // both x textSize
        assert_quad(
            &fit(IconTextFitType::Both, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -32.222_22,
            -6.111_110_7,
            12.222_221,
            16.111_11,
        );

        // both x textSize + padding
        assert_quad(
            &fit(IconTextFitType::Both, [5.0, 10.0, 5.0, 10.0], 12.0 / 24.0),
            -43.333_332,
            -11.666_666,
            23.333_332,
            21.666_666,
        );

        // both x textSize + padding t/r/b/l
        // (C++ also sets `layout.get<TextSize>() = 12.0f` on an unused layout.)
        assert_quad(
            &fit(IconTextFitType::Both, [0.0, 5.0, 10.0, 15.0], 12.0 / 24.0),
            -48.333_332,
            -6.666_666,
            18.333_332,
            26.666_666,
        );
    }
}
