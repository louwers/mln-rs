use glam::Vec2;
use mln_style::image::ImageStretch;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::math::deg2radf;
use mln_style::types::{SymbolPlacementType, TextRotationAlignmentType};
use mln_util::constants::ONE_EM;

use crate::text::PositionedIcon;
use crate::text::atlas::{ImagePosition, ImagePositions, RectU16};
use crate::text::glyph::Shaping;

/// A glyph's or an icon's quad, relative to its symbol's anchor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolQuad {
    pub(crate) tl: Vec2,
    pub(crate) tr: Vec2,
    pub(crate) bl: Vec2,
    pub(crate) br: Vec2,
    /// The quad's image in the atlas.
    pub(crate) tex: RectU16,
    /// The offsets, in pixels, of a stretched icon's corners that don't
    /// scale with its size.
    pub(crate) pixel_offset_tl: Vec2,
    pub(crate) pixel_offset_br: Vec2,
    /// The glyph's offset along a line it follows.
    pub(crate) glyph_offset: Vec2,
    pub(crate) is_sdf: bool,
    pub(crate) section_index: usize,
    /// The smallest scale of a stretched icon, where its fixed parts fill it.
    pub(crate) min_font_scale: Vec2,
}

pub(crate) type SymbolQuads = Vec<SymbolQuad>;

/// The border around an image in the atlas: its padding.
const BORDER: u16 = ImagePosition::PADDING;

/// The buffer around a glyph in its rect that its metrics don't include: the
/// glyph SDF's border of 3 and the atlas padding of 1.
const GLYPH_RECT_BUFFER: f32 = 4.0;

/// The total length of `stretches` within `min..max`.
fn stretch_within(stretches: &[ImageStretch], min: f32, max: f32) -> f32 {
    stretches.iter().map(|&(start, end)| end.clamp(min, max) - start.clamp(min, max)).sum()
}

/// A position along one axis of an icon's image: `stretch` pixels of its
/// stretchable parts and `fixed` pixels of the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cut {
    fixed: f32,
    stretch: f32,
}

impl Cut {
    /// The position in the image, in pixels.
    fn texel(self) -> f32 {
        self.stretch + self.fixed
    }
}

/// How one axis of an icon's image maps to its quad: the stretchable parts
/// stretch with the icon's size, the rest keeps its size in pixels.
struct StretchAxis<'a> {
    stretches: &'a [ImageStretch],
    /// The image's stretchable and fixed pixels.
    stretch_size: f32,
    fixed_size: f32,
    /// The stretchable and fixed pixels before the icon's content, and
    /// within it. Without content, the content is the whole image.
    stretch_offset: f32,
    stretch_content: f32,
    fixed_offset: f32,
    fixed_content: f32,
    /// Where the icon starts on this axis, and its size, in ems.
    icon_start: f32,
    icon_size: f32,
}

impl<'a> StretchAxis<'a> {
    fn new(
        stretches: &'a [ImageStretch],
        image_size: f32,
        content: Option<(f32, f32)>,
        icon_start: f32,
        icon_size: f32,
    ) -> Self {
        let stretch_size: f32 = stretches.iter().map(|&(start, end)| end - start).sum();
        let fixed_size = image_size - stretch_size;
        let (stretch_offset, stretch_content, fixed_offset, fixed_content) = match content {
            None => (0.0, stretch_size, 0.0, fixed_size),
            Some((start, end)) => {
                let stretch_offset = stretch_within(stretches, 0.0, start);
                let stretch_content = stretch_within(stretches, start, end);
                (stretch_offset, stretch_content, start - stretch_offset, end - start - stretch_content)
            }
        };
        StretchAxis {
            stretches,
            stretch_size,
            fixed_size,
            stretch_offset,
            stretch_content,
            fixed_offset,
            fixed_content,
            icon_start,
            icon_size,
        }
    }

    /// Where `cut` is in the quad, in ems.
    fn em(&self, cut: Cut) -> f32 {
        self.icon_start + self.icon_size * (cut.stretch - self.stretch_offset) / self.stretch_content
    }

    /// How far `cut` is from where [`em`](Self::em) puts it, in image pixels.
    fn px(&self, cut: Cut) -> f32 {
        cut.fixed - self.fixed_offset - self.fixed_content * cut.stretch / self.stretch_size
    }

    /// The cuts at the image's edges, including its border, and at the
    /// edges of its stretchable parts.
    fn cuts(&self) -> Vec<Cut> {
        let mut cuts = vec![Cut { fixed: -f32::from(BORDER), stretch: 0.0 }];
        let mut stretched = 0.0;
        for &(start, end) in self.stretches {
            cuts.push(Cut { fixed: start - stretched, stretch: stretched });
            cuts.push(Cut { fixed: start - stretched, stretch: stretched + (end - start) });
            stretched += end - start;
        }
        cuts.push(Cut { fixed: self.fixed_size + f32::from(BORDER), stretch: self.stretch_size });
        cuts
    }
}

/// The quads of `shaped_icon`, rotated by `icon_rotate` degrees. An icon
/// fitted to its text with stretchable parts is cut into a quad for each
/// part, so that only the stretchable ones stretch.
#[must_use]
pub(crate) fn icon_quads(shaped_icon: &PositionedIcon, icon_rotate: f32, has_icon_text_fit: bool) -> SymbolQuads {
    let image = &shaped_icon.image;
    let pixel_ratio = image.pixel_ratio;
    let image_width = f32::from(image.padded_rect.w - 2 * BORDER);
    let image_height = f32::from(image.padded_rect.h - 2 * BORDER);
    let full_x = [(0.0, image_width)];
    let full_y = [(0.0, image_height)];
    let stretch_x = if image.stretch_x.is_empty() { &full_x[..] } else { &image.stretch_x };
    let stretch_y = if image.stretch_y.is_empty() { &full_y[..] } else { &image.stretch_y };

    let content = image.content.filter(|_| has_icon_text_fit);
    let bounds = if has_icon_text_fit { shaped_icon.text_fit_bounds() } else { shaped_icon.bounds };

    let x = StretchAxis::new(stretch_x, image_width, content.map(|c| (c.left, c.right)), bounds.left, bounds.width());
    let y = StretchAxis::new(stretch_y, image_height, content.map(|c| (c.top, c.bottom)), bounds.top, bounds.height());

    let rotation = (icon_rotate != 0.0).then(|| Vec2::from_angle(deg2radf(icon_rotate)));
    let rotate = |p: Vec2| rotation.map_or(p, |rotation| rotation.rotate(p));
    let min_font_scale =
        Vec2::new(x.fixed_content, y.fixed_content) / pixel_ratio / Vec2::new(x.icon_size, y.icon_size);
    let image_origin = Vec2::new(f32::from(image.padded_rect.x + BORDER), f32::from(image.padded_rect.y + BORDER));

    let quad = |left: Cut, top: Cut, right: Cut, bottom: Cut| {
        let (x1, y1) = (left.texel(), top.texel());
        let (x2, y2) = (right.texel(), bottom.texel());
        SymbolQuad {
            tl: rotate(Vec2::new(x.em(left), y.em(top))),
            tr: rotate(Vec2::new(x.em(right), y.em(top))),
            bl: rotate(Vec2::new(x.em(left), y.em(bottom))),
            br: rotate(Vec2::new(x.em(right), y.em(bottom))),
            tex: RectU16 {
                x: (image_origin.x + x1) as u16,
                y: (image_origin.y + y1) as u16,
                w: (x2 - x1) as u16,
                h: (y2 - y1) as u16,
            },
            pixel_offset_tl: Vec2::new(x.px(left), y.px(top)) / pixel_ratio,
            pixel_offset_br: Vec2::new(x.px(right), y.px(bottom)) / pixel_ratio,
            glyph_offset: Vec2::ZERO,
            is_sdf: image.sdf,
            section_index: 0,
            min_font_scale,
        }
    };

    if !has_icon_text_fit || (image.stretch_x.is_empty() && image.stretch_y.is_empty()) {
        // The whole image, and the border around it.
        let start = Cut { fixed: 0.0, stretch: -1.0 };
        vec![quad(
            start,
            start,
            Cut { fixed: 0.0, stretch: image_width + 1.0 },
            Cut { fixed: 0.0, stretch: image_height + 1.0 },
        )]
    } else {
        let (x_cuts, y_cuts) = (x.cuts(), y.cuts());
        x_cuts
            .windows(2)
            .flat_map(|x| y_cuts.windows(2).map(move |y| (x, y)))
            .map(|(x, y)| quad(x[0], y[0], x[1], y[1]))
            .collect()
    }
}

/// The quads of the glyphs and images in `shaped_text`. A glyph without
/// area has none.
#[must_use]
pub(crate) fn glyph_quads(
    shaped_text: &Shaping,
    text_offset: [f32; 2],
    layout: &SymbolLayoutProperties::Values,
    placement: SymbolPlacementType,
    icons: &ImagePositions,
    allow_vertical_placement: bool,
) -> SymbolQuads {
    let text_rotation = (layout.text_rotate != 0.0).then(|| Vec2::from_angle(deg2radf(layout.text_rotate)));
    let along_line = layout.text_rotation_alignment != TextRotationAlignmentType::Viewport
        && placement != SymbolPlacementType::Point;

    let mut quads = SymbolQuads::new();

    for line in &shaped_text.lines {
        for glyph in &line.glyphs {
            let rect = glyph.rect;
            if rect.w == 0 || rect.h == 0 {
                continue;
            }

            let (rect_buffer, pixel_ratio, is_sdf) = match &glyph.image_id {
                Some(image_id) => {
                    let Some(image) = icons.get(image_id) else {
                        continue;
                    };
                    (f32::from(ImagePosition::PADDING) / image.pixel_ratio, image.pixel_ratio, image.sdf)
                }
                None => (GLYPH_RECT_BUFFER, 1.0, true),
            };

            let rotate_vertical_glyph = (along_line || allow_vertical_placement) && glyph.vertical;
            let half_advance = glyph.metrics.advance as f32 * glyph.scale / 2.0;

            // Align images and scaled glyphs in the middle of a vertical line.
            let line_offset = if allow_vertical_placement && shaped_text.verticalizable {
                let scaled_glyph_offset = (glyph.scale - 1.0) * ONE_EM;
                let image_offset = (ONE_EM - glyph.metrics.width as f32 * glyph.scale) / 2.0;
                line.line_offset / 2.0 - if glyph.image_id.is_some() { -image_offset } else { scaled_glyph_offset }
            } else {
                0.0
            };

            let glyph_offset = if along_line { Vec2::new(glyph.x + half_advance, glyph.y) } else { Vec2::ZERO };

            let label_offset = if along_line {
                Vec2::ZERO
            } else {
                Vec2::new(glyph.x + half_advance + text_offset[0], glyph.y + text_offset[1] - line_offset)
            };
            // A vertical glyph is rotated first and moved to its offset after.
            let (built_in_offset, verticalized_label_offset) =
                if rotate_vertical_glyph { (Vec2::ZERO, label_offset) } else { (label_offset, Vec2::ZERO) };

            // The rect buffer is in 1x logical units; only rect.w/h carry the
            // texture scale. Fractional left/top metrics keep 2x glyphs
            // centered.
            let texture_scale: f32 = if glyph.metrics.is_double_resolution { 2.0 } else { 1.0 };
            let x1 = (glyph.metrics.left - rect_buffer) * glyph.scale - half_advance + built_in_offset.x;
            let y1 = (-glyph.metrics.top - rect_buffer) * glyph.scale + built_in_offset.y;
            let x2 = x1 + f32::from(rect.w) / texture_scale * glyph.scale / pixel_ratio;
            let y2 = y1 + f32::from(rect.h) / texture_scale * glyph.scale / pixel_ratio;
            let mut corners = [Vec2::new(x1, y1), Vec2::new(x2, y1), Vec2::new(x1, y2), Vec2::new(x2, y2)];

            if rotate_vertical_glyph {
                // Vertical-supporting glyphs are laid out in 24x24 point boxes
                // (1 square em) In horizontal orientation, the y values for
                // glyphs are below the midline and we use a y offset of -17 to
                // pull them up to the middle. By rotating counter-clockwise
                // around the point at the center of the left edge of a 24x24
                // layout box centered below the midline, we align the center of
                // the glyphs with the horizontal midline, so the y offset is no
                // longer necessary, but we also pull the glyph to the left
                // along the x axis. The y coordinate includes the baseline's
                // y offset, which needs to be accounted for when the glyph is
                // rotated and translated.
                let center = Vec2::new(-half_advance, half_advance - Shaping::Y_OFFSET);

                // The x half-width offset correction is the difference
                // between full-width and half-width advance: 0 for full-width
                // glyphs, and it pulls up half-width glyphs.
                let x_half_width_offset_correction = ONE_EM / 2.0 - half_advance;
                let y_image_offset_correction =
                    if glyph.image_id.is_some() { x_half_width_offset_correction } else { 0.0 };
                let offset_correction =
                    Vec2::new(5.0 - Shaping::Y_OFFSET - x_half_width_offset_correction, -y_image_offset_correction);

                corners = corners.map(|p| {
                    // A quarter turn counter-clockwise, with y down.
                    let d = p - center;
                    Vec2::new(d.y, -d.x) + center + offset_correction + verticalized_label_offset
                });
            }

            if let Some(rotation) = text_rotation {
                corners = corners.map(|p| rotation.rotate(p));
            }

            let [tl, tr, bl, br] = corners;
            quads.push(SymbolQuad {
                tl,
                tr,
                bl,
                br,
                tex: rect,
                pixel_offset_tl: Vec2::ZERO,
                pixel_offset_br: Vec2::ZERO,
                glyph_offset,
                is_sdf,
                section_index: glyph.section_index,
                min_font_scale: Vec2::ZERO,
            });
        }
    }

    quads
}

#[cfg(test)]
mod tests {
    use mln_style::bitmap::PremultipliedImage;
    use mln_style::image::Impl as ImageImpl;
    use mln_style::size::Size;
    use mln_style::types::{IconTextFitType, SymbolAnchorType};

    use super::*;
    use crate::text::glyph::{Bounds, GlyphID, GlyphMetrics, PositionedGlyph, PositionedLine};

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
        PositionedGlyph { glyph: GlyphID::new(code), scale: 1.0, rect, metrics, ..Default::default() }
    }

    // Atlas padding is not scaled with the texture scale, so a 2x glyph quad
    // is 1 logical pixel smaller than its 1x equivalent. Fractional metrics
    // keep its center aligned with the 1x quad.
    // gtest: getGlyphQuads.DoubleResolutionGlyphPreservesLogicalQuadSize
    #[test]
    fn glyph_quads_double_resolution_glyph_preserves_logical_quad_size() {
        let metrics1x =
            GlyphMetrics { width: 24, height: 24, left: 0.0, top: -8.0, advance: 24, is_double_resolution: false };

        let mut metrics2x = metrics1x;
        metrics2x.left += 0.5;
        metrics2x.top -= 0.5;
        metrics2x.is_double_resolution = true;

        // Rects reflect the real DynamicTextureAtlas output: bitmap.size + 2 *
        // ImagePosition::padding (= 2). 1x bitmap 30 -> rect 32; 2x bitmap 60 -> rect 62.
        let shaped_text1x = Shaping {
            lines: vec![PositionedLine {
                glyphs: vec![positioned_glyph('A' as u32, rect(0, 0, 32, 32), metrics1x)],
                line_offset: 0.0,
            }],
            ..Default::default()
        };

        let shaped_text2x = Shaping {
            lines: vec![PositionedLine {
                glyphs: vec![positioned_glyph('A' as u32, rect(0, 0, 62, 62), metrics2x)],
                line_offset: 0.0,
            }],
            ..Default::default()
        };

        let layout = SymbolLayoutProperties::Values::default();
        let image_map = ImagePositions::default();
        let quads1x = glyph_quads(&shaped_text1x, [0.0, 0.0], &layout, SymbolPlacementType::Point, &image_map, false);
        let quads2x = glyph_quads(&shaped_text2x, [0.0, 0.0], &layout, SymbolPlacementType::Point, &image_map, false);

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
    fn icon_quads_normal() {
        let image = ImagePosition::new(rect(0, 0, 15, 11), &test_image_impl());

        let shaped_icon = PositionedIcon::new(&image, [-6.5, -4.5], SymbolAnchorType::Center);

        let quads = icon_quads(&shaped_icon, 0.0, false);

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
    fn icon_quads_style() {
        let image = ImagePosition::new(rect(0, 0, 20, 20), &test_image_impl());

        let shaped_text = Shaping {
            lines: vec![PositionedLine {
                glyphs: vec![positioned_glyph(32, RectU16::default(), GlyphMetrics::default())],
                line_offset: 0.0,
            }],
            bounds: Bounds { left: -60.0, top: -10.0, right: 20.0, bottom: 30.0 },
            ..Default::default()
        };

        let fit = |text_fit: IconTextFitType, padding: [f32; 4], font_scale: f32| {
            let mut shaped_icon = PositionedIcon::new(&image, [-9.5, -9.5], SymbolAnchorType::Center);
            shaped_icon.fit_to_text(&shaped_text, text_fit, padding, [0.0, 0.0], font_scale);
            let quads = icon_quads(&shaped_icon, 0.0, false);
            assert_eq!(quads.len(), 1);
            quads[0].clone()
        };

        // none
        {
            let shaped_icon = PositionedIcon::new(&image, [-9.5, -9.5], SymbolAnchorType::Center);

            assert_float_eq(-18.5, shaped_icon.bounds.top);
            assert_float_eq(-0.5, shaped_icon.bounds.right);
            assert_float_eq(-0.5, shaped_icon.bounds.bottom);
            assert_float_eq(-18.5, shaped_icon.bounds.left);

            let quads = icon_quads(&shaped_icon, 0.0, false);

            assert_eq!(quads.len(), 1);
            assert_quad(&quads[0], -19.5, -19.5, 0.5, 0.5);
        }

        // width
        assert_quad(&fit(IconTextFitType::Width, [0.0, 0.0, 0.0, 0.0], 24.0 / 24.0), -64.444_44, 0.0, 24.444_443, 20.0);

        // width x text size
        assert_quad(
            &fit(IconTextFitType::Width, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -32.222_22,
            -5.0,
            12.222_221,
            15.0,
        );

        // width x text size + padding
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

        // height x text size
        assert_quad(
            &fit(IconTextFitType::Height, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -20.0,
            -6.111_110_7,
            0.0,
            16.111_11,
        );

        // height x text size + padding
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

        // both x text size
        assert_quad(
            &fit(IconTextFitType::Both, [0.0, 0.0, 0.0, 0.0], 12.0 / 24.0),
            -32.222_22,
            -6.111_110_7,
            12.222_221,
            16.111_11,
        );

        // both x text size + padding
        assert_quad(
            &fit(IconTextFitType::Both, [5.0, 10.0, 5.0, 10.0], 12.0 / 24.0),
            -43.333_332,
            -11.666_666,
            23.333_332,
            21.666_666,
        );

        // both x text size + padding t/r/b/l
        assert_quad(
            &fit(IconTextFitType::Both, [0.0, 5.0, 10.0, 15.0], 12.0 / 24.0),
            -48.333_332,
            -6.666_666,
            18.333_332,
            26.666_666,
        );
    }

    #[test]
    fn icon_quads_cut_a_stretchable_icon_at_its_stretches() {
        let image = ImageImpl::new(
            "stretchable".to_owned(),
            PremultipliedImage::new(Size { width: 10, height: 10 }),
            1.0,
            false,
            vec![(2.0, 8.0)],
            vec![(2.0, 8.0)],
            Some(mln_style::image::ImageContent { left: 2.0, top: 2.0, right: 8.0, bottom: 8.0 }),
            None,
            None,
        )
        .expect("valid image");
        let position = ImagePosition::new(rect(20, 30, 12, 12), &image);
        let shaped_icon = PositionedIcon::new(&position, [0.0, 0.0], SymbolAnchorType::Center);

        let quads = icon_quads(&shaped_icon, 0.0, true);

        // Three columns and rows: fixed, stretched and fixed, which together
        // cover the padded image.
        let texs: Vec<_> = quads.iter().map(|quad| quad.tex).collect();
        assert_eq!(texs.len(), 9);
        assert_eq!(texs[0], rect(20, 30, 3, 3));
        assert_eq!(texs[4], rect(23, 33, 6, 6));
        assert_eq!(texs[8], rect(29, 39, 3, 3));
        // A fixed corner has no size in ems, only in pixels: from the border
        // to the content's start.
        assert_eq!(quads[0].tl, quads[0].br);
        assert_eq!(quads[0].pixel_offset_tl, Vec2::new(-3.0, -3.0));
        assert_eq!(quads[0].pixel_offset_br, Vec2::ZERO);
    }
}
