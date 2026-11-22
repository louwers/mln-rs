use mln_gfx::draw_mode::DrawMode;
use mln_map_types::View;
use mln_render::draw::{Buffer, Composite, Depth, Draw, DrawBlocks, Draws, IndexBuffer, Mesh, MeshDesc};
use mln_render::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, HitTest, Layer, LayerBucket, LayerValues, NewCx, PaintVertices,
    PrepareCx, TileCx, TileTransform, UpdateCx,
};
use mln_style::evaluated::Evaluated;
use mln_util::Color;
use mln_util::intersection_tests::polygon_intersects_multi_polygon;

use super::bucket::{FillBinders, FillBucket, LINE_WIDTH, draws_outline_quads};
use super::layout::FillLayout;
use super::programs::{
    FILL, FILL_OUTLINE, FILL_OUTLINE_INSTANCED, FILL_OUTLINE_PATTERN, FILL_OUTLINE_PATTERN_INSTANCED,
    FILL_OUTLINE_PULLED, FILL_PATTERN,
};
use super::ubo::{
    FillDrawableUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO, FillPatternTilePropsUBO,
};
use super::{Fill, FillPaintProperties};
use crate::features::PATTERN_SAMPLER;

/// A fill layer: each tile's polygons filled with a color or a pattern, and
/// outlined.
pub struct FillLayer {
    /// The quad that instanced outlines draw an instance of per segment,
    /// its corners x the segment's start (0) or end (1) and y its side, and
    /// its triangle strip.
    corners: Buffer<[i16; 2]>,
    strip: IndexBuffer,
    /// Whether the fills of the tiles made in the frame write depth, as the
    /// first of them says.
    fills_write: Option<bool>,
}

/// A tile's fill and outline, each where it has one.
pub enum FillTile {
    /// Of a layer without a pattern, whose fill writes depth where `write`,
    /// and whose outline finds its paint values by `paint_layout` where it
    /// reads them itself.
    Color {
        fill: Option<Mesh<FillDrawableUBO>>,
        outline: Option<Mesh<FillOutlineDrawableUBO>>,
        write: bool,
        paint_layout: u32,
    },
    /// Of a layer with a pattern.
    Pattern { fill: Option<Mesh<PatternBlocks>>, outline: Option<Mesh<PatternBlocks>> },
}

type PatternBlocks = (FillPatternDrawableUBO, FillPatternTilePropsUBO);

impl Layer for FillLayer {
    type Style = Fill;
    type Input = Features<FillLayout>;
    type Tile = FillTile;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let corners = cx.res.buffer(&[[0, 0], [0, 1], [1, 0], [1, 1]]);
        FillLayer { corners, strip: cx.res.index_buffer(&[0, 1, 2, 3]), fills_write: None }
    }

    /// An undefined outline color is the fill color.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let (layer, paint) = (cx.layer, &mut cx.evaluated.paint);
        if layer.unevaluated.fill_outline_color.is_undefined() {
            paint.fill_outline_color = paint.fill_color.clone();
        }
        if *paint.fill_layer_opacity.uniform() <= 0.0 {
            Evaluation::NONE
        } else if layer.unevaluated.fill_pattern.is_undefined() && opaque(&paint.fill_color, &paint.fill_opacity) {
            Evaluation::OPAQUE
        } else {
            Evaluation::TRANSLUCENT
        }
    }

    /// The frame's first tile made says anew whether the frame's fills
    /// write depth.
    fn update(&mut self, _cx: &mut UpdateCx<'_, Self>) {
        self.fills_write = None;
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, Fill, FillBucket>,
    ) -> Option<FillTile> {
        let (bucket, values) = (laid.bucket, &laid.laid_with.paint);
        if bucket.triangles.is_empty() && bucket.lines.is_empty() {
            return None;
        }
        let pattern = has_pattern(cx.layer);
        if pattern {
            let (fill, outline) = self.meshes(cx, laid, true);
            return Some(FillTile::Pattern { fill, outline });
        }
        // Only opaque fills write depth.
        let write = *self.fills_write.get_or_insert_with(|| opaque(&values.fill_color, &values.fill_opacity));
        let (fill, outline) = self.meshes(cx, laid, false);
        let caps = cx.res.caps();
        let pulled = draws_outline_quads(caps) && caps.binds_vertex_buffers;
        let paint_layout = if pulled { paint_layout(laid.binders(&bucket.binders)) } else { 0 };
        Some(FillTile::Color { fill, outline, write, paint_layout })
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let (layer, frame) = (&*cx.layer.evaluated, cx.frame);
        let paint = &layer.paint;
        let main = out.main();
        if let Some(composite) = Composite::of(*paint.fill_layer_opacity.uniform(), *paint.fill_layer_blend.uniform()) {
            main.composite(composite);
        }
        let crossfade = &layer.crossfade;
        main.block(&FillEvaluatedPropsUBO {
            fade: crossfade.t,
            from_scale: crossfade.from_scale,
            to_scale: crossfade.to_scale,
            ..FillEvaluatedPropsUBO::default()
        });
        let how = transform(paint);
        let at_depth =
            |sub_layer, write| Draw { depth: Depth::Layer { sub_layer, write }, clip: true, ..Draw::DEFAULT };
        let faded = paint.fill_pattern.faded();
        let outline_is_fill_color = cx.layer.unevaluated.fill_outline_color.is_undefined();
        for (tile, laid, value) in cx.tiles() {
            let at = tile.project(frame, &how);
            match value {
                &FillTile::Color { ref fill, ref outline, write, paint_layout } => {
                    if let Some(fill) = fill {
                        main.push_at(fill, &at, at_depth(1, write), FillDrawableUBO::default());
                    }
                    if let Some(outline) = outline {
                        let draw = at_depth(if outline_is_fill_color { 2 } else { 0 }, false);
                        main.push_at(outline, &at, draw, FillOutlineDrawableUBO { paint_layout, ..Default::default() });
                    }
                }
                FillTile::Pattern { fill, outline } => {
                    // The shaders divide by the atlas's size.
                    let atlas = laid.icon_atlas().map(|atlas| cx.res.size(atlas)).filter(|size| !size.is_empty());
                    let Some(atlas) = atlas else { continue };
                    let origin = tile.pattern_origin(frame);
                    let blocks = (
                        FillPatternDrawableUBO {
                            pixel_coord_upper: origin.upper,
                            pixel_coord_lower: origin.lower,
                            tile_ratio: origin.tile_units_to_pixels,
                            ..Default::default()
                        },
                        FillPatternTilePropsUBO {
                            pattern_from: laid.pattern(faded.from.id()),
                            pattern_to: laid.pattern(faded.to.id()),
                            texsize: [atlas.width as f32, atlas.height as f32],
                            sdf: if laid.bucket.sdf(cx.layer.id()) { 1.0 } else { 0.0 },
                            ..Default::default()
                        },
                    );
                    if let Some(fill) = fill {
                        main.push_at(fill, &at, at_depth(1, true), blocks);
                    }
                    if let Some(outline) = outline {
                        main.push_at(outline, &at, at_depth(2, false), blocks);
                    }
                }
            }
        }
    }

    fn query_radius(&self, layer: &LayerValues<Fill>, _laid: LayerBucket<'_, Fill, FillBucket>, _camera: &View) -> f32 {
        transform(&layer.evaluated.paint).translation_length()
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let paint = &test.layer.evaluated.paint;
        polygon_intersects_multi_polygon(&test.query_at(&transform(paint)), test.geometry).then_some(0.0)
    }
}

impl FillLayer {
    /// The meshes of `laid`'s fill and outline, with or without a
    /// `pattern`, where the tile has them.
    fn meshes<F: DrawBlocks, O: DrawBlocks>(
        &self,
        cx: &mut TileCx<'_, Self>,
        laid: LayerBucket<'_, Fill, FillBucket>,
        pattern: bool,
    ) -> (Option<Mesh<F>>, Option<Mesh<O>>) {
        let (bucket, values, res) = (laid.bucket, &laid.laid_with.paint, &mut *cx.res);
        let (vertices, paint) = (laid.buffer(&bucket.vertices), laid.binders(&bucket.binders));
        let atlas = laid.icon_atlas().filter(|_| pattern);
        let fill = (!bucket.triangles.is_empty()).then(|| {
            let mut desc = MeshDesc::new(if pattern { &FILL_PATTERN } else { &FILL }, laid.indexes(&bucket.triangles));
            desc.vertices(vertices).paint(paint);
            if let Some(atlas) = atlas {
                desc.texture(atlas, PATTERN_SAMPLER);
            }
            res.mesh(&desc)
        });

        // The outline draws in the fill color unless it has one of its
        // own, which a pattern doesn't default to.
        let outlined = *values.fill_antialias.uniform()
            && (!pattern || cx.layer.unevaluated.fill_outline_color.is_undefined())
            && !bucket.lines.is_empty();
        let quads = draws_outline_quads(res.caps());
        let outline = outlined.then(|| {
            let segments;
            let mut desc = if !quads {
                let program = if pattern { &FILL_OUTLINE_PATTERN } else { &FILL_OUTLINE };
                let mut desc = MeshDesc::new(program, laid.indexes(&bucket.lines));
                desc.primitive(DrawMode::Lines { width: LINE_WIDTH }).vertices(vertices);
                desc
            } else if res.caps().binds_vertex_buffers && !pattern {
                segments = res.quads(bucket.vertices.len());
                let mut desc = MeshDesc::new(&FILL_OUTLINE_PULLED, &segments);
                desc.pulled(vertices).pulled(laid.buffer(&bucket.outline_ends));
                if let Some(buffer) = paint.buffer() {
                    desc.pulled(buffer);
                }
                desc
            } else {
                let program = if pattern { &FILL_OUTLINE_PATTERN_INSTANCED } else { &FILL_OUTLINE_INSTANCED };
                let mut desc = MeshDesc::new(program, &self.strip);
                desc.primitive(DrawMode::TriangleStrip)
                    .vertices(&self.corners)
                    .instances(vertices)
                    .instances(laid.buffer(&bucket.outline_ends));
                desc
            };
            desc.paint(paint);
            if let Some(atlas) = atlas {
                desc.texture(atlas, PATTERN_SAMPLER);
            }
            res.mesh(&desc)
        });
        (fill, outline)
    }
}

/// Whether the layer fills with a pattern rather than its color.
fn has_pattern(layer: &LayerValues<Fill>) -> bool {
    !layer.unevaluated.fill_pattern.is_undefined()
}

/// Whether a fill of `color` at `opacity` covers what is behind it.
fn opaque(color: &Evaluated<Color>, opacity: &Evaluated<f32>) -> bool {
    color.constant_or(&Color::default()).a >= 1.0 && opacity.constant_or(&0.0) >= 1.0
}

/// Where the pulled outlines' shader finds a vertex's paint values among
/// `paint`'s ([`FillOutlineDrawableUBO::paint_layout`]): a byte each of
/// words, from the lowest, a vertex's, then the offsets of its outline color
/// and opacity.
fn paint_layout(paint: PaintVertices<'_, FillBinders>) -> u32 {
    let (color, opacity) = (paint.fill_outline_color.offset(), paint.fill_opacity.offset());
    let words = |bytes: usize| (bytes / 4) as u32 & 0xff;
    let stride = if color.is_some() || opacity.is_some() { words(paint.stride()) } else { 0 };
    stride | color.map_or(0, words) << 8 | opacity.map_or(0, words) << 16
}

/// How the layer's `paint` places a tile's fills: moved by its translate.
fn transform(paint: &FillPaintProperties::PossiblyEvaluated) -> TileTransform {
    TileTransform::translated(*paint.fill_translate.uniform(), *paint.fill_translate_anchor.uniform())
}
