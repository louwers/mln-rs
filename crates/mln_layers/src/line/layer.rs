use std::rc::Rc;

use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::View;
use mln_render::draw::{Composite, Draw, Draws, Mesh, MeshDesc};
use mln_render::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, HitTest, Layer, LayerBucket, LayerValues, NewCx, PrepareCx, TileCx,
    TileTransform, UpdateCx,
};
use mln_style::StyleValue as _;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates};
use mln_util::Color;
use mln_util::intersection_tests::polygon_intersects_buffered_multi_line;
use mln_util::size::Size;

use super::bucket::LineBucket;
use super::dashes::{DASH_SAMPLER, DashTextures};
use super::layout::LineLayout;
use super::programs::{LINE, LINE_GRADIENT, LINE_GRADIENT_SDF, LINE_PATTERN, LINE_SDF};
use super::ubo::{
    LineDrawableUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO, LinePatternTilePropsUBO,
    LineSDFDrawableUBO, LineSDFTilePropsUBO,
};
use super::{Line, LinePaintProperties};
use crate::color_ramp::ColorRamp;
use crate::features::PATTERN_SAMPLER;

/// A line layer: each tile's lines, in a color, along a gradient, with a
/// pattern, or dashed.
pub struct LineLayer {
    /// The color ramp of `line-gradient`.
    ramp: ColorRamp,
    /// The patterns of the layer's dash array, where it is the same for
    /// every feature.
    dashes: DashTextures,
}

/// A tile's mesh of its lines, drawn as the layer draws them.
pub enum LineTile {
    Plain(Mesh<LineDrawableUBO>),
    Gradient(Mesh<LineGradientDrawableUBO>),
    Pattern(Mesh<(LinePatternDrawableUBO, LinePatternTilePropsUBO)>),
    Dashed(Mesh<(LineSDFDrawableUBO, LineSDFTilePropsUBO)>),
}

/// How a line samples its gradient's color ramp.
const RAMP_SAMPLER: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

impl Layer for LineLayer {
    type Style = Line;
    type Input = Features<LineLayout>;
    type Tile = LineTile;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        LineLayer { ramp: ColorRamp::new(cx.res, 256), dashes: DashTextures::default() }
    }

    /// Makes the gradient's color ramp again where its value may have
    /// changed.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        self.ramp.evaluate(cx.layer.unevaluated.line_gradient.get_value(), cx.parameters);
        let paint = &cx.evaluated.paint;
        Evaluation::translucent_if(
            *paint.line_layer_opacity.uniform() > 0.0
                && paint.line_opacity.constant_or(&1.0) > 0.0
                && paint.line_color.constant_or(&Color::black()).a > 0.0
                && paint.line_width.constant_or(&1.0) > 0.0,
        )
    }

    /// Uploads a new color ramp, and makes the textures of the dash array's
    /// patterns that the frame's dashed tiles draw with.
    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        self.ramp.update(cx.res);
        let dasharray = cx.layer.evaluated.paint.line_dasharray.faded();
        for i in 0..cx.tiles.len() {
            let (_, Some(laid), _) = cx.tiles.get_mut(i) else { continue };
            let laid_dasharray = &laid.laid_with.paint.line_dasharray;
            if laid_dasharray.is_constant() && !laid_dasharray.faded().from.is_empty() {
                self.dashes.set(cx.res, dasharray.from, dasharray.to, laid.bucket.dash_cap);
            }
        }
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, Line, LineBucket>,
    ) -> Option<LineTile> {
        let (bucket, values, res) = (laid.bucket, &laid.laid_with.paint, &mut *cx.res);
        if bucket.triangles.is_empty() {
            return None;
        }
        let (vertices, paint) = (laid.buffer(&bucket.vertices), laid.binders(&bucket.binders));
        let desc = |program| {
            let mut desc = MeshDesc::new(program, laid.indexes(&bucket.triangles));
            desc.vertices(vertices).paint(paint, values);
            desc
        };
        let gradient = !cx.layer.unevaluated.line_gradient.get_value().is_undefined();
        let ramp = gradient.then(|| self.ramp.texture());
        let dasharray = &values.line_dasharray;
        if !dasharray.is_constant() || !dasharray.faded().from.is_empty() {
            let feature_dashes = bucket.dashes(cx.layer.id());
            let dashes = match feature_dashes {
                Some(dashes) => {
                    let image = dashes.atlas.image();
                    if !image.valid() {
                        return None;
                    }
                    res.texture(|texture| texture.set_alpha_image(Rc::new(image.clone())))
                }
                None => self.dashes.get(bucket.dash_cap)?.texture.clone(),
            };
            let mut desc = desc(if gradient { &LINE_GRADIENT_SDF } else { &LINE_SDF });
            if let Some(dashes) = feature_dashes {
                desc.vertices(laid.buffer(&dashes.vertices));
            }
            if let Some(ramp) = ramp {
                desc.texture(ramp, RAMP_SAMPLER);
            }
            desc.texture(&dashes, DASH_SAMPLER);
            return Some(LineTile::Dashed(res.mesh(&desc)));
        }
        if !cx.layer.unevaluated.line_pattern.is_undefined() {
            let mut desc = desc(&LINE_PATTERN);
            desc.texture(laid.icon_atlas()?, PATTERN_SAMPLER);
            return Some(LineTile::Pattern(res.mesh(&desc)));
        }
        if let Some(ramp) = ramp {
            let mut desc = desc(&LINE_GRADIENT);
            desc.texture(ramp, RAMP_SAMPLER);
            return Some(LineTile::Gradient(res.mesh(&desc)));
        }
        Some(LineTile::Plain(res.mesh(&desc(&LINE))))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let (layer, frame) = (&*cx.layer.evaluated, cx.frame);
        let (paint, crossfade, camera) = (&layer.paint, &layer.crossfade, frame.camera);
        let main = out.main();
        if let Some(composite) = Composite::of(*paint.line_layer_opacity.uniform(), *paint.line_layer_blend.uniform()) {
            main.composite(composite);
        }
        // The pattern's program reads the floor width, which isn't one of
        // its properties.
        let floorwidth = paint.line_floor_width.constant_or(&LinePaintProperties::Defaults::line_floor_width());
        main.block(&LineEvaluatedPropsUBO { floorwidth, ..LineEvaluatedPropsUBO::default() });
        let how = transform(paint);
        let (zoom, integer_zoom) = (camera.zoom() as f32, f32::from(camera.integer_zoom()));
        let pixel_scale = camera.projection().pixel_scale(camera) as f32;
        let draw = Draw { clip: true, ..Draw::DEFAULT };
        for (tile, laid, value) in cx.tiles() {
            let at = tile.project(frame, &how);
            let id = tile.unwrapped_id();
            let ratio = pixel_scale / id.pixels_to_tile_units(1.0, zoom);
            let tile_ratio = |scale: f32| 1.0 / id.pixels_to_tile_units(scale, integer_zoom);
            match value {
                LineTile::Plain(mesh) => {
                    main.push_at(mesh, &at, draw, LineDrawableUBO { ratio, ..Default::default() });
                }
                LineTile::Gradient(mesh) => {
                    main.push_at(mesh, &at, draw, LineGradientDrawableUBO { ratio, ..Default::default() });
                }
                LineTile::Pattern(mesh) => {
                    let atlas = laid.icon_atlas().map_or_else(Size::default, |atlas| cx.res.size(atlas));
                    let pattern = paint.line_pattern.faded();
                    let binders = laid.binders(&laid.bucket.binders);
                    let blocks = (
                        LinePatternDrawableUBO {
                            ratio,
                            floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                            ..Default::default()
                        },
                        LinePatternTilePropsUBO {
                            pattern_from: laid.pattern(pattern.from.id()),
                            pattern_to: laid.pattern(pattern.to.id()),
                            scale: [frame.pixel_ratio, tile_ratio(1.0), crossfade.from_scale, crossfade.to_scale],
                            texsize: [atlas.width as f32, atlas.height as f32],
                            fade: crossfade.t,
                            ..Default::default()
                        },
                    );
                    main.push_at(mesh, &at, draw, blocks);
                }
                LineTile::Dashed(mesh) => {
                    let mut drawable = LineSDFDrawableUBO { ratio, ..Default::default() };
                    let sdfgamma = if let Some(dashes) = laid.bucket.dashes(cx.layer.id()) {
                        // The patterns differ by feature, which an attribute
                        // reads: both samples are of the tile's zoom's
                        // pattern, at the cross-fade's two scales.
                        let size = dashes.atlas.image().size;
                        drawable.patternscale_a = [tile_ratio(crossfade.from_scale), 0.0];
                        drawable.patternscale_b = [tile_ratio(crossfade.to_scale), 0.0];
                        drawable.atlas_height = size.height as f32;
                        let scale = crossfade.from_scale.min(crossfade.to_scale);
                        size.width as f32 / (scale * 256.0 * frame.pixel_ratio) / 2.0
                    } else {
                        let Some(dashes) = self.dashes.get(laid.bucket.dash_cap) else { continue };
                        let (from, to) = (dashes.from, dashes.to);
                        let (width_a, width_b) = (from.width * crossfade.from_scale, to.width * crossfade.to_scale);
                        drawable.patternscale_a = [tile_ratio(width_a), -from.height / 2.0];
                        drawable.patternscale_b = [tile_ratio(width_b), -to.height / 2.0];
                        drawable.tex_y_a = from.y;
                        drawable.tex_y_b = to.y;
                        let width = if width_b < width_a { width_b } else { width_a };
                        dashes.size.width as f32 / (width * 256.0 * frame.pixel_ratio) / 2.0
                    };
                    let tile_props = LineSDFTilePropsUBO { sdfgamma, mix: crossfade.t, ..Default::default() };
                    main.push_at(mesh, &at, draw, (drawable, tile_props));
                }
            }
        }
    }

    fn query_radius(&self, layer: &LayerValues<Line>, laid: LayerBucket<'_, Line, LineBucket>, _camera: &View) -> f32 {
        type Defaults = LinePaintProperties::Defaults;
        let (paint, binders) = (&layer.evaluated.paint, laid.binders(&laid.bucket.binders));
        // A data-driven value's largest over the bucket's features, or else
        // its constant value or default.
        let offset = binders.line_offset.max().or(paint.line_offset.constant()).unwrap_or(Defaults::line_offset());
        let width = binders.line_width.max().or(paint.line_width.constant()).unwrap_or(Defaults::line_width());
        let gap = binders.line_gap_width.max().or(paint.line_gap_width.constant());
        let gap = gap.unwrap_or(Defaults::line_gap_width());
        let width = if gap == 0.0 { width } else { gap + 2.0 * width };
        width / 2.0 + offset.abs() + transform(paint).translation_length()
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let paint = &test.layer.evaluated.paint;
        let query = test.query_at(&transform(paint));
        let offset = test.feature_value(&paint.line_offset) * test.pixels_to_tile_units;
        let (width, gap) = (test.feature_value(&paint.line_width), test.feature_value(&paint.line_gap_width));
        let width = if gap == 0.0 { width } else { gap + 2.0 * width };
        let half_width = (f64::from(width) / 2.0 * f64::from(test.pixels_to_tile_units)) as f32;
        let hit = if offset != 0.0 && !test.geometry.is_empty() {
            polygon_intersects_buffered_multi_line(&query, &offset_lines(test.geometry, f64::from(offset)), half_width)
        } else {
            polygon_intersects_buffered_multi_line(&query, test.geometry, half_width)
        };
        hit.then_some(0.0)
    }
}

/// `lines` moved `offset` tile units to their left, each vertex along the
/// bisector of its segments, rounded toward zero.
fn offset_lines(lines: &GeometryCollection, offset: f64) -> Vec<GeometryCoordinates> {
    use geometry::Point;
    use mln_util::math::{perp, unit};

    let to_f64 = |p: Point<i16>| Point::new(f64::from(p.x), f64::from(p.y));
    let sub = |a: Point<i16>, b: Point<i16>| Point::new(a.x.wrapping_sub(b.x), a.y.wrapping_sub(b.y));
    let zero = Point::new(0.0, 0.0);
    lines
        .iter()
        .map(|line| {
            (0..line.len())
                .map(|i| {
                    let p = line[i];
                    let a_to_b = if i == 0 { zero } else { perp(unit(to_f64(sub(p, line[i - 1])))) };
                    // The last vertex takes the incoming normal, so that the
                    // end moves the whole offset.
                    let b_to_c = if i + 1 == line.len() { a_to_b } else { perp(unit(to_f64(sub(line[i + 1], p)))) };
                    let extrude = unit(Point::new(a_to_b.x + b_to_c.x, a_to_b.y + b_to_c.y));
                    let cos_half_angle = extrude.x * b_to_c.x + extrude.y * b_to_c.y;
                    let scale = if cos_half_angle == 0.0 { 0.0 } else { 1.0 / cos_half_angle };
                    // Tile units wrap.
                    let shifted = Point::new((extrude.x * scale * offset) as i16, (extrude.y * scale * offset) as i16);
                    Point::new(shifted.x.wrapping_add(p.x), shifted.y.wrapping_add(p.y))
                })
                .collect()
        })
        .collect()
}

/// How the layer's `paint` places a tile's lines: moved by its translate.
fn transform(paint: &LinePaintProperties::PossiblyEvaluated) -> TileTransform {
    TileTransform::translated(*paint.line_translate.uniform(), *paint.line_translate_anchor.uniform())
}
