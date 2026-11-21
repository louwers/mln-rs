use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_render::draw::{
    AtlasBinding, Composite, Context, DrawGeometry, DrawList, DrawObjects, DrawObjectsMut, FrameDraw, IconFilter,
    LayerDrawing, SharedAttribute, SharedTexture, Texture2D, Textures, TileDraws, TileObjects, UniformWriter, Uniforms,
    VertexAttributeArray, bucket_indexes, bucket_vertices, cast_matrix, paint_vertices, projection_data,
    to_projection_ubo,
};
use mln_render::layer::{
    DrawContext, EvaluatedLayer, FeatureQuery, HitTest, LayerBucket, LayerType, LayerValues, LayoutParameters,
    RenderPass, SourceLayer, UpdateContext,
};
use mln_style::expression::{Dependency, EvaluationContext, GlobalStateMap, global_state_refs_intersect};
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_style_types::LineCapType;
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::bucket::{LineBinders, LineBucket, LineLayoutSpec};
use super::dashes::{DashCap, DashTextures};
use super::mesh::LineLayoutVertex;
use super::programs::{
    ID_LINE_DASH_TEXTURE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_IMAGE_TEXTURE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_TILE_PROPS_UBO,
};
use super::properties::LinePaintProperties;
use super::ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use super::{Line, programs};
use crate::color_ramp::apply_color_ramp;
use crate::pattern_layout::{PatternLayout, uses_pattern};
use crate::refresh_paint_attributes;

/// What a line draws: its shader and the uniforms it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineType {
    Simple,
    Gradient,
    Pattern,
    /// Dashes.
    Sdf,
    /// Dashes over a gradient.
    GradientSdf,
}

/// What the layer keeps of a tile: the draw of its bucket's lines, as it was
/// built for the bucket.
pub struct LineTile {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    line_type: LineType,
    /// The passes the draw draws in.
    pass: RenderPass,
    /// The ends of the dashes of the bucket, which pick its dash texture.
    dash_cap: DashCap,
    /// Gives a pattern draw the texture of the tile's icon atlas.
    atlases: Option<AtlasBinding>,
}

impl TileObjects for LineTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        Some(DrawObjectsMut { objects: &mut self.objects, atlases: self.atlases.as_ref() })
    }

    fn drop_bindings(&mut self) {
        self.objects.drop_bindings();
    }
}

/// A draw's entries in the layer's storage buffers.
struct LineUniforms {
    draw_ubo: LineDrawableUnionUBO,
    tile_props: LineTilePropsUnionUBO,
    projection: ProjectionUBO,
}

/// Where a dash draw samples its patterns. Where the dash array differs by
/// feature, the shaders divide the scales' x and the gamma by each pattern's
/// length.
struct SdfPatterns {
    patternscale_a: [f32; 2],
    patternscale_b: [f32; 2],
    tex_y_a: f32,
    tex_y_b: f32,
    atlas_height: f32,
    sdfgamma: f32,
}

fn cast_tlbr(a: [u16; 4]) -> [f32; 4] {
    a.map(f32::from)
}

/// The ends of a bucket's dashes. A dash pattern is shared by the whole
/// bucket, so a `line-cap` that varies by feature gets square dashes; each
/// line still gets its own cap at its ends.
fn dash_pattern_cap(bucket: &LineBucket) -> DashCap {
    if bucket.layout.line_cap.constant_or(&LineCapType::Butt) == LineCapType::Round {
        DashCap::Round
    } else {
        DashCap::Square
    }
}

/// A texture of the bucket's dash patterns, for a `line-dasharray` that
/// differs by feature.
fn dash_atlas_texture(gpu: &mut dyn Gpu, context: &mut Context, binders: &LineBinders) -> Option<SharedTexture> {
    let image = binders.line_dasharray.atlas()?.image();
    if !image.valid() {
        return None;
    }
    let texture = context.add_texture_with(|texture| {
        texture.set_format(TextureFormat::A8Unorm);
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Repeat,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        });
    });
    if let Err(error) = context.upload_texture_data(gpu, &texture, &image.data, image.size) {
        Log::error(Event::Render, format!("Uploading the dash patterns failed: {error}"));
        return None;
    }
    Some(texture)
}

/// What a line layer keeps besides its drawing.
pub struct LineState {
    /// The frame's draws as the update makes them.
    draw_list: DrawList<LineUniforms>,

    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,
    color_ramp_texture: Option<SharedTexture>,
    /// Whether the color ramp changed since the draws that read it were
    /// built, which the next update drops.
    color_ramp_changed: bool,
    /// The textures of the dash patterns of the layer's `line-dasharray`,
    /// where it doesn't differ by feature.
    dashes: DashTextures,
}

impl LineState {
    /// Makes the color ramp of `layer`'s `line-gradient` again.
    fn update_color_ramp(&mut self, layer: &LayerValues<Line>) {
        let color_value = layer.unevaluated.line_gradient.get_value().clone();
        let mut image = (*self.color_ramp).clone();
        if !apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref()) {
            return;
        }
        // The texture shares the ramp image, so a new one replaces it.
        self.color_ramp = Rc::new(image);

        if self.color_ramp_texture.is_some() {
            self.color_ramp_texture = None;
            self.color_ramp_changed = true;
        }
    }
}

/// The width a layer with `layer`'s values draws `feature` at `zoom`, in
/// `feature_state`, with its gap.
fn line_width(
    layer: &EvaluatedLayer<Line>,
    feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
    zoom: f32,
    feature_state: &mln_style::expression::expr::FeatureState,
) -> f32 {
    let evaluated = &layer.paint;
    let line_width = evaluated.line_width.evaluate(&EvaluationContext::with_zoom_feature_state(
        zoom,
        Some(feature),
        Some(feature_state),
    ));
    let gap_width = evaluated.line_gap_width.evaluate(&EvaluationContext::with_zoom_feature_state(
        zoom,
        Some(feature),
        Some(feature_state),
    ));
    if gap_width == 0.0 { line_width } else { gap_width + 2.0 * line_width }
}

// query
fn offset_line(
    rings: &mln_tile_id::geometry_tile_data::GeometryCollection,
    offset: f64,
) -> Vec<mln_tile_id::geometry_tile_data::GeometryCoordinates> {
    use geometry::Point;
    use mln_util::math::{perp, unit};

    debug_assert!(offset != 0.0);
    debug_assert!(!rings.is_empty());

    let to_f64 = |p: Point<i16>| Point::new(f64::from(p.x), f64::from(p.y));
    let sub = |a: Point<i16>, b: Point<i16>| Point::new(a.x.wrapping_sub(b.x), a.y.wrapping_sub(b.y));

    let zero = Point::new(0.0, 0.0);
    let mut new_rings = Vec::with_capacity(rings.len());
    for ring in rings.iter() {
        let mut new_ring = Vec::with_capacity(ring.len());

        for i in 0..ring.len() {
            let p = ring[i];

            let a_to_b = if i == 0 { zero } else { perp(unit(to_f64(sub(p, ring[i - 1])))) };
            // At the last vertex, use the incoming normal so the endpoint receives the full offset.
            let b_to_c = if i + 1 == ring.len() { a_to_b } else { perp(unit(to_f64(sub(ring[i + 1], p)))) };
            let mut extrude = unit(Point::new(a_to_b.x + b_to_c.x, a_to_b.y + b_to_c.y));

            let cos_half_angle = extrude.x * b_to_c.x + extrude.y * b_to_c.y;
            let scale = if cos_half_angle == 0.0 { 0.0 } else { 1.0 / cos_half_angle };
            extrude = Point::new(extrude.x * scale, extrude.y * scale);

            // The offset rounds toward zero to tile units, which wrap.
            let shifted = Point::new((extrude.x * offset) as i16, (extrude.y * offset) as i16);
            new_ring.push(Point::new(shifted.x.wrapping_add(p.x), shifted.y.wrapping_add(p.y)));
        }
        new_rings.push(new_ring);
    }

    new_rings
}

impl LayerType for Line {
    type Layout = PatternLayout<LineLayoutSpec>;
    type Bucket = LineBucket;
    type View<'a> = LayerBucket<'a, Line>;
    type Drawing = TileDraws<LineTile>;
    type State = LineState;
    const CROSSFADES: bool = true;

    /// Whether a layer of the group has a pattern.
    fn layout_can_need_dependencies(group: &[Arc<EvaluatedLayer<Line>>]) -> bool {
        uses_pattern::<LineLayoutSpec>(group)
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Line>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<PatternLayout<LineLayoutSpec>>, Error> {
        PatternLayout::new(parameters, group, source_layer).map(Some)
    }

    fn feature_query() -> Option<&'static dyn FeatureQuery<Line>> {
        Some(&Line)
    }

    fn new_layer(_layer: &LayerValues<Line>) -> (LineState, TileDraws<LineTile>) {
        let state = LineState {
            draw_list: DrawList::default(),
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            color_ramp_texture: None,
            color_ramp_changed: false,
            dashes: DashTextures::default(),
        };
        (state, TileDraws::default())
    }

    fn complete_evaluation(
        state: &mut LineState,
        layer: &LayerValues<Line>,
        parameters: &PropertyEvaluationParameters,
        _evaluated: &mut EvaluatedLayer<Line>,
    ) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&state.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = layer.unevaluated.line_gradient.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        state.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            state.update_color_ramp(layer);
        }
    }

    fn passes(_layer: &LayerValues<Line>, evaluated: &EvaluatedLayer<Line>) -> RenderPass {
        let evaluated = &evaluated.paint;
        if *evaluated.line_layer_opacity.uniform() > 0.0
            && evaluated.line_opacity.constant_or(&1.0) > 0.0
            && evaluated.line_color.constant_or(&Color::black()).a > 0.0
            && evaluated.line_width.constant_or(&1.0) > 0.0
        {
            RenderPass::TRANSLUCENT
        } else {
            RenderPass::NONE
        }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn make_draws(
        state: &mut LineState,
        drawing: &mut TileDraws<LineTile>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Line>,
    ) {
        let render_tiles = cx.tiles;
        state.dashes.upload(gpu, cx.context);
        if std::mem::take(&mut state.color_ramp_changed) {
            // The gradients' draws read the old ramp's texture.
            drawing.clear();
        }

        let frame = drawing.frame_mut();
        let paint = &cx.layer.evaluated.paint;
        frame.set_composite(Composite::of(*paint.line_layer_opacity.uniform(), *paint.line_layer_blend.uniform()));
        frame.set_stencil_tiles(render_tiles.iter().map(|tile| tile.unwrapped_id()));

        // Lines draw only in the translucent pass.
        let render_pass = cx.layer.evaluated.passes() & RenderPass::TRANSLUCENT;
        for tile in render_tiles {
            let tile_id = tile.id();

            let Some(render_data) = tile.view_for_pass(render_pass) else {
                drawing.tiles_mut().drop_value(&tile_id);
                continue;
            };
            let bucket = render_data.bucket;
            if bucket.triangles.is_empty() {
                drawing.tiles_mut().drop_value(&tile_id);
                continue;
            }
            let Some(binders) = bucket.paint_property_binders.get(cx.layer.id()) else {
                continue;
            };
            let evaluated = &render_data.layer.paint;

            let Some(mut entry) = drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };
            if entry.value.as_ref().is_some_and(|cached| cached.pass != render_pass) {
                // The tile's draw was built for other passes.
                *entry.value = None;
            }
            // A draw built from another bucket goes.
            entry.set_bucket(bucket.base.id());

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *cx.context;
            let cached = if let Some(cached) = entry.value {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    let buffer = &binders.interleaved_vertex_buffer;
                    let paint =
                        paint_vertices(context, render_data.geometry(), buffer, binders.depends_on_feature_state());
                    binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
                });
                cached
            } else {
                let built = state.build_tile(
                    cx.layer,
                    cx.projection_variant,
                    gpu,
                    context,
                    render_data,
                    binders,
                    evaluated,
                    render_pass,
                );
                let Some(cached) = built.and_then(|built| drawing.tiles_mut().insert(&tile_id, built).ok()) else {
                    continue;
                };
                cached
            };
            let tile = tile_id.to_unwrapped();
            if let Some(uniforms) = LineState::uniforms(tile, render_data, binders, cached, &state.dashes, cx) {
                let draw = FrameDraw { pass: cached.pass, clip: true, ..FrameDraw::new(Some(tile_id), cached.id) };
                state.draw_list.push(draw, uniforms);
            }
        }

        let layer = &cx.layer.evaluated;
        drawing.finish(&mut state.draw_list, gpu, cx.context, |writer| {
            LineState::write_uniforms(layer, writer);
        });
    }

    /// Makes the dash textures of the tiles' buckets, then the layer's own,
    /// which the encode draws a dashed tile with when its bucket has no
    /// per-feature patterns.
    fn update(state: &mut LineState, _drawing: &mut TileDraws<LineTile>, cx: &mut UpdateContext<'_, '_, Line>) {
        let (dashes, tiles) = (&mut state.dashes, cx.tiles);
        for tile in tiles {
            let Some(bucket) = tile.view() else {
                continue;
            };
            let dasharray = bucket.layer.paint.line_dasharray.faded();
            if dasharray.from.is_empty() {
                continue;
            }
            let cap = dash_pattern_cap(bucket.bucket);
            dashes.add(dasharray.from, dasharray.to, cap);
        }
        // A dashed draw asks for the layer's pattern even where it is empty.
        let layer_dasharray = cx.layer.evaluated.paint.line_dasharray.faded();
        for tile in tiles {
            let Some(bucket) = tile.view() else {
                continue;
            };
            let laid_out = &bucket.layer.paint.line_dasharray;
            let dashed = !laid_out.is_constant() || !laid_out.faded().from.is_empty();
            let per_feature = bucket
                .bucket
                .paint_property_binders
                .get(cx.layer.id())
                .is_some_and(|binders| binders.line_dasharray.atlas().is_some());
            if dashed && !per_feature {
                let cap = dash_pattern_cap(bucket.bucket);
                dashes.add(layer_dasharray.from, layer_dasharray.to, cap);
            }
        }
    }
}

impl FeatureQuery<Line> for Line {
    fn query_radius(&self, bucket: &LineBucket, layer: &EvaluatedLayer<Line>) -> f32 {
        bucket.query_radius(layer)
    }

    fn intersects(&self, layer: &EvaluatedLayer<Line>, test: &HitTest<'_>) -> bool {
        let HitTest { query_geometry, feature, geometry, zoom, pixels_to_tile_units, feature_state, .. } = *test;
        let evaluated = &layer.paint;
        // Translate query geometry
        let translated_query_geometry = test
            .translated_query_geometry(*evaluated.line_translate.uniform(), *evaluated.line_translate_anchor.uniform());
        let query = translated_query_geometry.as_ref().unwrap_or(query_geometry);

        // Evaluate function
        let offset = evaluated.line_offset.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        )) * pixels_to_tile_units;
        // Test intersection
        let half_width =
            (f64::from(line_width(layer, feature, zoom, feature_state)) / 2.0 * f64::from(pixels_to_tile_units)) as f32;

        // Apply offset to geometry
        if offset != 0.0 && !geometry.is_empty() {
            return mln_util::intersection_tests::polygon_intersects_buffered_multi_line(
                query,
                &offset_line(geometry, f64::from(offset)),
                half_width,
            );
        }

        mln_util::intersection_tests::polygon_intersects_buffered_multi_line(query, geometry, half_width)
    }
}

impl LineState {
    /// The draw of the tile's `bucket`, laid out with `evaluated`, in
    /// `pass` and the projection `variant`, of a layer of `layer`'s values,
    /// if the layer has what it draws with.
    #[expect(clippy::too_many_arguments, reason = "the layer, the tile's bucket and how the draw draws it")]
    fn build_tile(
        &mut self,
        layer: &LayerValues<Line>,
        variant: ProjectionVariant,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        tile_bucket: LayerBucket<'_, Line>,
        binders: &LineBinders,
        evaluated: &LinePaintProperties::PossiblyEvaluated,
        pass: RenderPass,
    ) -> Option<LineTile> {
        let (bucket, geometry) = (tile_bucket.bucket, tile_bucket.geometry());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        let mut uniforms = Uniforms::default();
        let mut attrs = VertexAttributeArray::new();
        binders.bind(evaluated, paint.as_ref(), &mut attrs, &mut uniforms);

        let has_dasharray =
            !evaluated.line_dasharray.is_constant() || !evaluated.line_dasharray.faded().from.is_empty();
        // A constant dash array's texture is the layer's, which the draw's
        // uniforms take.
        let dash_texture = if evaluated.line_dasharray.is_constant() {
            None
        } else {
            Some(dash_atlas_texture(gpu, context, binders)?)
        };
        let has_gradient = !layer.unevaluated.line_gradient.get_value().is_undefined();

        let mut textures = Textures::default();
        let mut atlases = None;
        let (line_type, program) = if has_dasharray && has_gradient {
            textures[ID_LINE_IMAGE_TEXTURE] = Some(self.color_ramp_texture(context)?);
            textures[ID_LINE_DASH_TEXTURE] = dash_texture;
            (LineType::GradientSdf, &programs::LINE_GRADIENT_SDF)
        } else if has_dasharray {
            textures[ID_LINE_IMAGE_TEXTURE] = dash_texture;
            (LineType::Sdf, &programs::LINE_SDF)
        } else if !layer.unevaluated.line_pattern.is_undefined() {
            let icons = Some(tile_bucket.atlas_textures()).filter(|atlases| atlases.icon().is_some())?;
            atlases = Some(AtlasBinding::new(
                Some(icons.clone()),
                None,
                Some(ID_LINE_IMAGE_TEXTURE),
                /*is_text*/ false,
                IconFilter::Linear,
            ));
            (LineType::Pattern, &programs::LINE_PATTERN)
        } else if has_gradient {
            textures[ID_LINE_IMAGE_TEXTURE] = Some(self.color_ramp_texture(context)?);
            (LineType::Gradient, &programs::LINE_GRADIENT)
        } else {
            (LineType::Simple, &programs::LINE)
        };
        let shader = context.shader_key(program, uniforms, variant);
        if let Some(atlases) = &atlases {
            // A pattern binds the atlas's texture and no other.
            textures = atlases.textures(&shader);
        }

        let shared = bucket_vertices(geometry, &bucket.vertices)?;
        let stride = size_of::<LineLayoutVertex>() as u32;
        attrs.set_shared(
            ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE,
            SharedAttribute::new(
                shared.clone(),
                AttributeDataType::Short2,
                offset_of!(LineLayoutVertex, a1) as u32,
                stride,
            ),
        );
        attrs.set_shared(
            ID_LINE_DATA_VERTEX_ATTRIBUTE,
            SharedAttribute::new(shared, AttributeDataType::UByte4, offset_of!(LineLayoutVertex, a2) as u32, stride),
        );

        let draw_geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() },
            mode: DrawMode::Triangles(),
            indexes: bucket_indexes(geometry, &bucket.triangles)?.into(),
            segments: bucket.segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        let color_mode = context.color_mode(if pass == RenderPass::TRANSLUCENT {
            ColorMode::alpha_blended()
        } else {
            ColorMode::unblended()
        });
        let objects = DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), draw_geometry, textures);
        Some(LineTile {
            id: SimpleIdentity::new(),
            objects,
            line_type,
            pass,
            dash_cap: dash_pattern_cap(bucket),
            atlases,
        })
    }

    /// The texture of the color ramp, which every gradient of the layer
    /// shares, if the ramp is valid.
    fn color_ramp_texture(&mut self, context: &mut Context) -> Option<SharedTexture> {
        if self.color_ramp_texture.is_none() && self.color_ramp.valid() {
            self.color_ramp_texture = Some(context.add_texture_with(|texture| {
                texture.set_image(Some(Rc::clone(&self.color_ramp)));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }));
        }
        self.color_ramp_texture.clone()
    }

    /// The entries in the layer's storage buffers of `cached`, the draw of
    /// the tile `tile_id` of `tile_bucket`, whose binders are `binders`, in
    /// the frame of `cx`: its matrix, the interpolation of the layer's
    /// values, and where its patterns are. A dashed draw of the layer's dash
    /// array takes its texture in `dashes` here; `None` where there is none
    /// yet, and doesn't draw.
    fn uniforms(
        tile_id: UnwrappedTileID,
        tile_bucket: LayerBucket<'_, Line>,
        binders: &LineBinders,
        cached: &mut LineTile,
        dashes: &DashTextures,
        cx: &DrawContext<'_, '_, Line>,
    ) -> Option<LineUniforms> {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let (layer, context, frame) = (&*cx.layer.evaluated, &*cx.context, &cx.frame);
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        let state = frame.view;
        let zoom = state.zoom() as f32;
        let int_zoom = state.integer_zoom();
        let pixel_scale = state.projection().pixel_scale(state) as f32;

        let projection = projection_data(
            &tile_id,
            frame,
            *evaluated.line_translate.uniform(),
            *evaluated.line_translate_anchor.uniform(),
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let matrix = cast_matrix(&projection.fallback_matrix);
        let mut uniforms = LineUniforms {
            draw_ubo: LineDrawableUnionUBO::default(),
            tile_props: LineTilePropsUnionUBO::default(),
            projection: to_projection_ubo(&projection),
        };
        let ratio = pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom);

        match cached.line_type {
            LineType::Simple => uniforms.draw_ubo.set(&LineDrawableUBO {
                matrix,
                ratio,
                color_t: binders.line_color.interpolation_factor(zoom),
                blur_t: binders.line_blur.interpolation_factor(zoom),
                opacity_t: binders.line_opacity.interpolation_factor(zoom),
                gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                offset_t: binders.line_offset.interpolation_factor(zoom),
                width_t: binders.line_width.interpolation_factor(zoom),
                pad1: 0.0,
            }),
            LineType::Gradient => uniforms.draw_ubo.set(&LineGradientDrawableUBO {
                matrix,
                ratio,
                blur_t: binders.line_blur.interpolation_factor(zoom),
                opacity_t: binders.line_opacity.interpolation_factor(zoom),
                gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                offset_t: binders.line_offset.interpolation_factor(zoom),
                width_t: binders.line_width.interpolation_factor(zoom),
                pad1: 0.0,
                pad2: 0.0,
            }),
            LineType::Pattern => {
                let texture =
                    cached.objects.texture(ID_LINE_IMAGE_TEXTURE).and_then(|texture| context.texture(texture));
                let texture_size = texture.map_or(Size::new(0, 0), Texture2D::size);
                uniforms.draw_ubo.set(&LinePatternDrawableUBO {
                    matrix,
                    ratio,
                    blur_t: binders.line_blur.interpolation_factor(zoom),
                    opacity_t: binders.line_opacity.interpolation_factor(zoom),
                    gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                    offset_t: binders.line_offset.interpolation_factor(zoom),
                    width_t: binders.line_width.interpolation_factor(zoom),
                    pattern_from_t: binders.line_pattern.interpolation_factor(zoom).0,
                    pattern_to_t: binders.line_pattern.interpolation_factor(zoom).1,
                    floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                    pad1: 0.0,
                    pad2: 0.0,
                    pad3: 0.0,
                });
                let pattern = evaluated.line_pattern.faded();
                let tlbr = |id: &str| tile_bucket.pattern(id).map_or([0.0; 4], |position| cast_tlbr(position.tlbr()));
                uniforms.tile_props.set(&LinePatternTilePropsUBO {
                    pattern_from: tlbr(pattern.from.id()),
                    pattern_to: tlbr(pattern.to.id()),
                    scale: [
                        frame.pixel_ratio,
                        1.0 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom)),
                        crossfade.from_scale,
                        crossfade.to_scale,
                    ],
                    texsize: [texture_size.width as f32, texture_size.height as f32],
                    fade: crossfade.t,
                    pad1: 0.0,
                });
            }
            LineType::Sdf | LineType::GradientSdf => {
                // The gradient keeps its ramp in the image slot, so the dash
                // pattern takes the second.
                let dash_texture_id = if cached.line_type == LineType::GradientSdf {
                    ID_LINE_DASH_TEXTURE
                } else {
                    ID_LINE_IMAGE_TEXTURE
                };
                let tile_ratio = |scale: f32| 1.0 / tile_id.pixels_to_tile_units(scale, f32::from(int_zoom));
                let sdf = if let Some(atlas) = binders.line_dasharray.atlas() {
                    // Per-feature patterns don't fade from the zoom before:
                    // both samples are of this zoom's pattern, at the
                    // cross-fade's two scales.
                    let atlas_size = atlas.image().size;
                    let min_scale = crossfade.from_scale.min(crossfade.to_scale);
                    SdfPatterns {
                        patternscale_a: [tile_ratio(crossfade.from_scale), 0.0],
                        patternscale_b: [tile_ratio(crossfade.to_scale), 0.0],
                        tex_y_a: 0.0,
                        tex_y_b: 0.0,
                        atlas_height: atlas_size.height as f32,
                        sdfgamma: atlas_size.width as f32 / (min_scale * 256.0 * frame.pixel_ratio) / 2.0,
                    }
                } else {
                    let dasharray = evaluated.line_dasharray.faded();
                    let dash_pattern = dashes.get(dasharray.from, dasharray.to, cached.dash_cap)?;
                    cached.objects.set_texture(dash_texture_id, Some(dash_pattern.texture()?));

                    let pos_a = dash_pattern.from();
                    let pos_b = dash_pattern.to();
                    let width_a = pos_a.width * crossfade.from_scale;
                    let width_b = pos_b.width * crossfade.to_scale;
                    let min_width = if width_b < width_a { width_b } else { width_a };
                    SdfPatterns {
                        patternscale_a: [tile_ratio(width_a), -pos_a.height / 2.0],
                        patternscale_b: [tile_ratio(width_b), -pos_b.height / 2.0],
                        tex_y_a: pos_a.y,
                        tex_y_b: pos_b.y,
                        atlas_height: 0.0,
                        sdfgamma: dash_pattern.size().width as f32 / (min_width * 256.0 * frame.pixel_ratio) / 2.0,
                    }
                };

                uniforms.draw_ubo.set(&LineSDFDrawableUBO {
                    matrix,
                    patternscale_a: sdf.patternscale_a,
                    patternscale_b: sdf.patternscale_b,
                    tex_y_a: sdf.tex_y_a,
                    tex_y_b: sdf.tex_y_b,
                    ratio,
                    color_t: binders.line_color.interpolation_factor(zoom),
                    blur_t: binders.line_blur.interpolation_factor(zoom),
                    opacity_t: binders.line_opacity.interpolation_factor(zoom),
                    gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                    offset_t: binders.line_offset.interpolation_factor(zoom),
                    width_t: binders.line_width.interpolation_factor(zoom),
                    floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                    atlas_height: sdf.atlas_height,
                    pad2: 0.0,
                });
                uniforms.tile_props.set(&LineSDFTilePropsUBO {
                    sdfgamma: sdf.sdfgamma,
                    mix: crossfade.t,
                    pad1: 0.0,
                    pad2: 0.0,
                });
            }
        }
        Some(uniforms)
    }

    /// Writes the layer's buffers of the frame's draws.
    fn write_uniforms(layer: &EvaluatedLayer<Line>, writer: &mut UniformWriter<'_, LineUniforms>) {
        let evaluated = &layer.paint;
        let props = LineEvaluatedPropsUBO {
            color: evaluated.line_color.constant_or(&LinePaintProperties::Defaults::line_color()).to_float_array(),
            blur: evaluated.line_blur.constant_or(&LinePaintProperties::Defaults::line_blur()),
            opacity: evaluated.line_opacity.constant_or(&LinePaintProperties::Defaults::line_opacity()),
            gapwidth: evaluated.line_gap_width.constant_or(&LinePaintProperties::Defaults::line_gap_width()),
            offset: evaluated.line_offset.constant_or(&LinePaintProperties::Defaults::line_offset()),
            width: evaluated.line_width.constant_or(&LinePaintProperties::Defaults::line_width()),
            floorwidth: evaluated.line_floor_width.constant_or(&LinePaintProperties::Defaults::line_floor_width()),
            pad0: 0.0,
            pad1: 0.0,
        };
        writer.uniform(ID_LINE_EVALUATED_PROPS_UBO, &props);
        writer.storage(ID_LINE_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
        writer.storage(ID_LINE_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
        writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
    }
}
