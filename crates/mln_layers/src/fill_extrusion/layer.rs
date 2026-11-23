use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureWrapType;
use mln_map_types::View;
use mln_render::draw::{
    Buffer, Composite, Depth, Draw, Draws, IndexBuffer, Mesh, MeshDesc, Resources, Sampler, Texture,
};
use mln_render::layer::{
    EvaluateCx, EvaluatedLayer, EvaluatedLight, Evaluation, Features, Frame, FrameTile, HitTest, Layer, LayerBucket,
    LayerValues, NewCx, PrepareCx, TileCx, TileTransform, UpdateCx,
};
use mln_style_types::{LayerBlendType, LightAnchorType};
use mln_util::constants::TILE_SIZE_D;
use mln_util::intersection_tests::polygon_intersects_multi_polygon;

use super::bucket::{FillExtrusionBucket, WALL_CORNERS};
use super::layout::FillExtrusionLayout;
use super::programs::{
    FILL_EXTRUSION, FILL_EXTRUSION_INSTANCED, FILL_EXTRUSION_PATTERN, FILL_EXTRUSION_PATTERN_INSTANCED,
    FILL_EXTRUSION_PATTERN_WALL, FILL_EXTRUSION_PULLED, FILL_EXTRUSION_WALL,
};
use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use super::{FillExtrusion, FillExtrusionPaintProperties};

/// How far behind its surfaces the layer's draws into depth alone write
/// their depth, in the clip depth that runs from 0 to 1: 16 steps of a
/// 24-bit depth buffer.
///
/// Every tile whose buffer reaches a wall draws it, each with its own
/// matrix, which rounds the wall's depth its own way in the last bits, and
/// places a pattern on it its own way. Were the depth pass exact, a draw in
/// color would pass the depth test only where its own tile's copy rounded
/// nearest, and a patterned wall would mix the tiles' patterns in bands that
/// depend on the GPU. Set back, every copy passes, and the first to draw a
/// pixel, which the layer's stencil then keeps, draws all of it. Surfaces
/// closer than this draw in the order of their draws rather than of their
/// depth.
const DEPTH_PASS_SETBACK: f64 = 1.0 / (1 << 20) as f64;

/// A fill-extrusion layer: each tile's roofs and walls, drawn into the
/// depth of the frame's 3D geometry.
pub struct FillExtrusionLayer {
    /// The wall quad, which instanced walls draw an instance of per outline
    /// vertex, and its triangle strip.
    corners: Buffer<[i16; 2]>,
    strip: IndexBuffer,
    /// How the tiles' patterns sample their atlas in the frame.
    patterns: Sampler,
}

/// The meshes of a tile's roofs or walls: in color, and into depth only,
/// which a translucent layer draws first, so that it blends only its
/// nearest surface over each pixel.
pub struct Part {
    color: Mesh<Blocks>,
    depth: Mesh<Blocks>,
}

type Blocks = (FillExtrusionDrawableUBO, FillExtrusionTilePropsUBO);

impl Layer for FillExtrusionLayer {
    type Style = FillExtrusion;
    type Input = Features<FillExtrusionLayout>;
    /// The tile's roofs, then its walls.
    type Tile = Vec<Part>;
    const VOLUME: bool = true;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        FillExtrusionLayer {
            corners: cx.res.buffer(&WALL_CORNERS),
            strip: cx.res.index_buffer(&[0, 2, 1, 3]),
            patterns: Sampler::new(SamplerState::default()),
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::translucent_if(opacity(cx.evaluated) > 0.0)
    }

    /// Filters the patterns as the frame asks.
    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        self.patterns.set(SamplerState {
            filter: cx.frame.image_filter(),
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..SamplerState::default()
        });
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, FillExtrusion, FillExtrusionBucket>,
    ) -> Option<Vec<Part>> {
        let (bucket, res) = (laid.bucket, &mut *cx.res);
        let pattern = has_pattern(cx.layer);
        let atlas = laid.icon_atlas().filter(|_| pattern).map(|atlas| (atlas, &self.patterns));
        let (vertices, paint) = (laid.buffer(&bucket.vertices), laid.binders(&bucket.binders));
        let mut roofs = MeshDesc::new(
            if pattern { &FILL_EXTRUSION_PATTERN } else { &FILL_EXTRUSION },
            laid.indexes(&bucket.triangles),
        );
        roofs.vertices(vertices).paint(paint);
        let mut parts = vec![part(res, &mut roofs, atlas)];

        if bucket.walls() > 0 {
            let quads = res.quads(bucket.walls());
            let mut walls = if let Some(walls) = &bucket.wall_quads {
                let program = if pattern { &FILL_EXTRUSION_PATTERN_WALL } else { &FILL_EXTRUSION_WALL };
                let mut desc = MeshDesc::new(program, &quads);
                desc.vertices(laid.buffer(&walls.vertices)).paint(laid.binders(&walls.binders));
                desc
            } else if res.caps().binds_vertex_buffers && !pattern {
                let mut desc = MeshDesc::new(&FILL_EXTRUSION_PULLED, &quads);
                desc.pulled(vertices);
                if let Some(buffer) = paint.buffer() {
                    desc.pulled(buffer);
                }
                desc.paint(paint);
                desc
            } else {
                let program = if pattern { &FILL_EXTRUSION_PATTERN_INSTANCED } else { &FILL_EXTRUSION_INSTANCED };
                let mut desc = MeshDesc::new(program, &self.strip);
                desc.primitive(DrawMode::TriangleStrip)
                    .vertices(&self.corners)
                    .instances(vertices)
                    .instances_from(vertices, 1)
                    .paint(paint);
                desc
            };
            parts.push(part(res, &mut walls, atlas));
        }
        Some(parts)
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let (layer, frame) = (&*cx.layer.evaluated, cx.frame);
        let paint = &layer.paint;
        let (opacity, pattern) = (opacity(layer), has_pattern(cx.layer));
        let main = out.main();
        // A translucent layer without a pattern draws as one opaque
        // surface, which composites at its opacity, where it can, and
        // elsewhere into depth first.
        let composite = Composite { opacity, blend: LayerBlendType::Normal, frame_depth: true };
        let composited = opacity < 1.0 && !pattern && main.composite(composite);
        let depth_first = !composited && (opacity < 1.0 || pattern);
        main.block(&props(layer, frame.light, frame.camera, if composited { 1.0 } else { opacity }));

        let how = TileTransform { volume: true, ..transform(paint) };
        let set_back = TileTransform { setback: DEPTH_PASS_SETBACK, ..how };
        let faded = paint.fill_extrusion_pattern.faded();
        for (tile, laid, parts) in cx.tiles() {
            let at = tile.project(frame, &how);
            let behind = depth_first.then(|| tile.project(frame, &set_back));
            let atlas = laid.icon_atlas().filter(|_| pattern).map_or_else(Default::default, |atlas| cx.res.size(atlas));
            let tile_props = FillExtrusionTilePropsUBO {
                pattern_from: laid.pattern(faded.from.id()),
                pattern_to: laid.pattern(faded.to.id()),
                texsize: [atlas.width as f32, atlas.height as f32],
                ..FillExtrusionTilePropsUBO::default()
            };
            let paint = laid.binders(&laid.bucket.binders);
            let offsets = [
                paint.fill_extrusion_base.offset(),
                paint.fill_extrusion_height.offset(),
                paint.fill_extrusion_color.offset(),
            ];
            let paint_layout = paint_layout(paint.stride(), offsets);
            let blocks = (drawable(tile, frame, paint_layout), tile_props);
            for part in parts {
                if let Some(behind) = &behind {
                    let draw = Draw { order: 0, depth: Depth::Volume { write: true, once: false }, ..Draw::DEFAULT };
                    main.push_at(&part.depth, behind, draw, blocks);
                }
                let draw = Draw { order: 1, depth: Depth::Volume { write: true, once: depth_first }, ..Draw::DEFAULT };
                main.push_at(&part.color, &at, draw, blocks);
            }
        }
    }

    fn query_radius(
        &self,
        layer: &LayerValues<FillExtrusion>,
        _laid: LayerBucket<'_, FillExtrusion, FillExtrusionBucket>,
        _camera: &View,
    ) -> f32 {
        transform(&layer.evaluated.paint).translation_length()
    }

    fn hit(&self, test: &HitTest<'_, Self>) -> Option<f32> {
        let paint = &test.layer.evaluated.paint;
        polygon_intersects_multi_polygon(&test.query_at(&transform(paint)), test.geometry).then_some(0.0)
    }
}

/// The meshes of a part of `desc`, culled to the faces toward the camera,
/// sampling `atlas` where the layer has a pattern.
fn part<'a>(res: &mut Resources, desc: &mut MeshDesc<'a>, atlas: Option<(&'a Texture, &Sampler)>) -> Part {
    desc.cull(CullFaceMode::back_ccw());
    if let Some((atlas, sampler)) = atlas {
        desc.texture(atlas, sampler);
    }
    let color = res.mesh(desc);
    let depth = res.mesh(desc.blend(ColorMode::disabled()));
    Part { color, depth }
}

/// Whether the layer draws a pattern rather than its color.
fn has_pattern(layer: &LayerValues<FillExtrusion>) -> bool {
    !layer.unevaluated.fill_extrusion_pattern.is_undefined()
}

/// The layer's opacity: as `fill-extrusion-opacity` is the same for every
/// extrusion, both opacities apply to the layer drawn as one surface.
fn opacity(layer: &EvaluatedLayer<FillExtrusion>) -> f32 {
    layer.paint.fill_extrusion_layer_opacity.uniform() * layer.paint.fill_extrusion_opacity.uniform()
}

/// Where the pulled walls' shader finds a vertex's paint values, which take
/// `stride` bytes, with the base's, the height's and the color's at their
/// `offsets` where they differ by feature
/// ([`FillExtrusionDrawableUBO::paint_layout`]): a byte each of words, from
/// the lowest, a vertex's, then the offsets of its base, height and color.
fn paint_layout(stride: usize, offsets: [Option<usize>; 3]) -> u32 {
    let words = |bytes: Option<usize>| (bytes.unwrap_or(0) / 4) as u32 & 0xff;
    let [base, height, color] = offsets.map(words);
    words(Some(stride)) | base << 8 | height << 16 | color << 24
}

/// The values of a draw of `tile` in `frame`, whose pulled walls find their
/// paint values by `paint_layout`.
fn drawable(tile: FrameTile, frame: &Frame<'_>, paint_layout: u32) -> FillExtrusionDrawableUBO {
    let origin = tile.pattern_origin(frame);
    let tiles = 2f64.powi(i32::from(tile.id().canonical.z));
    FillExtrusionDrawableUBO {
        pixel_coord_upper: origin.upper,
        pixel_coord_lower: origin.lower,
        height_factor: (-tiles / TILE_SIZE_D / 8.0) as f32,
        tile_ratio: origin.tile_units_to_pixels,
        paint_layout,
        ..FillExtrusionDrawableUBO::default()
    }
}

/// The values that every draw of `layer` reads, lit by `light` and seen by
/// `camera`, at `opacity`.
fn props(
    layer: &EvaluatedLayer<FillExtrusion>,
    light: &EvaluatedLight,
    camera: &View,
    opacity: f32,
) -> FillExtrusionPropsUBO {
    type Defaults = FillExtrusionPaintProperties::Defaults;
    let paint = &layer.paint;
    let light_color = light.light_color.uniform();
    let position = glam::Vec3::from_array(light.light_position.uniform().get_cartesian());
    // A light anchored to the viewport turns with the map.
    let position = if *light.light_anchor.uniform() == LightAnchorType::Viewport {
        glam::Mat3::from_rotation_z(-camera.angle() as f32) * position
    } else {
        position
    };
    FillExtrusionPropsUBO {
        color: paint.fill_extrusion_color.constant_or(&Defaults::fill_extrusion_color()).to_float_array(),
        light_color: [light_color.r, light_color.g, light_color.b],
        light_position: position.to_array(),
        base: paint.fill_extrusion_base.constant_or(&Defaults::fill_extrusion_base()),
        height: paint.fill_extrusion_height.constant_or(&Defaults::fill_extrusion_height()),
        light_intensity: *light.light_intensity.uniform(),
        vertical_gradient: if *paint.fill_extrusion_vertical_gradient.uniform() { 1.0 } else { 0.0 },
        opacity,
        fade: layer.crossfade.t,
        from_scale: layer.crossfade.from_scale,
        to_scale: layer.crossfade.to_scale,
        ..FillExtrusionPropsUBO::default()
    }
}

/// How the layer's `paint` places a tile's extrusions: moved by its
/// translate.
fn transform(paint: &FillExtrusionPaintProperties::PossiblyEvaluated) -> TileTransform {
    TileTransform::translated(
        *paint.fill_extrusion_translate.uniform(),
        *paint.fill_extrusion_translate_anchor.uniform(),
    )
}

#[cfg(test)]
mod tests {
    use super::paint_layout;

    #[test]
    fn the_pulled_walls_paint_layout_packs_words_from_the_lowest_byte() {
        assert_eq!(8 | 2 << 16 | 4 << 24, paint_layout(32, [Some(0), Some(8), Some(16)]));
        assert_eq!(2 | 1 << 24, paint_layout(8, [None, None, Some(4)]));
    }
}
