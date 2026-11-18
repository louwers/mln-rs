use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::TextureFormat;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASH_TEXTURE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO,
    ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_IMAGE_TEXTURE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_TILE_PROPS_UBO,
    ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::expression::{Dependency, EvaluationContext, GlobalStateMap, global_state_refs_intersect};
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Line;
use mln_style::layers::line_layer_properties::LinePaintProperties;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_style_types::{AlignmentType, LineCapType};
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::{cast_matrix, refresh_paint_attributes};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::line_bucket::{LineBinders, LineBucket, LineLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::store::SharedTexture;
use crate::gfx::texture2d::Texture2D;
use crate::gfx::vertex_attribute::{SharedAttribute, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_composite::Composite;
use crate::layer_drawing::{DrawList, DrawObjectsMut, FrameDraw, TileDraws, TileObjects, UniformWriter};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{LineLayoutSpec, PatternLayout, uses_pattern};
use crate::line_atlas::{LineAtlas, LinePatternCap};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    Atlases, LayerBucket, LayerParts, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer,
    RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_tile::FrameTile;
use crate::tile_matrix::{projection_data, to_projection_ubo};

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
pub(crate) struct LineTile {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    line_type: LineType,
    /// The passes the draw draws in.
    pass: RenderPass,
    /// The ends of the dashes of the bucket, which pick its pattern in the
    /// line atlas.
    dash_cap: LinePatternCap,
    /// Gives a pattern draw the texture of the tile's icon atlas.
    atlases: Option<AtlasBinding>,
}

impl TileObjects for LineTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        Some(DrawObjectsMut { objects: &mut self.objects, atlases: self.atlases.as_ref() })
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
fn dash_pattern_cap(bucket: &LineBucket) -> LinePatternCap {
    if bucket.layout.line_cap.constant_or(&LineCapType::Butt) == LineCapType::Round {
        LinePatternCap::Round
    } else {
        LinePatternCap::Square
    }
}

pub(crate) fn apply_color_ramp(
    color_value: &mln_style::color_ramp_property_value::ColorRampPropertyValue,
    image: &mut PremultipliedImage,
    global_state: Option<&GlobalStateMap>,
) -> bool {
    if color_value.is_undefined() {
        return false;
    }

    let length = image.bytes();

    let mut i: u32 = 0;
    while (i as usize) < length {
        let color = color_value.evaluate_ramp(f64::from(i) / length as f64, global_state);
        let i_ = i as usize;
        image.data[i_] = (color.r * 255.0).floor() as u8;
        image.data[i_ + 1] = (color.g * 255.0).floor() as u8;
        image.data[i_ + 2] = (color.b * 255.0).floor() as u8;
        image.data[i_ + 3] = (color.a * 255.0).floor() as u8;
        i += 4;
    }
    true
}

fn read_line_attributes(
    attrs: &mut VertexAttributeArray,
    b: &LineBinders,
    shared: Option<&SharedVertices>,
    evaluated: &LinePaintProperties::PossiblyEvaluated,
    pau: &mut StringIDSetsPair,
) {
    let buffer = &b.interleaved_vertex_buffer;
    let mut id = ID_LINE_COLOR_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &b.line_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.line_color.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_blur,
        buffer,
        shared,
        &["a_blur"],
        evaluated.line_blur.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_opacity,
        buffer,
        shared,
        &["a_opacity"],
        evaluated.line_opacity.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_gap_width,
        buffer,
        shared,
        &["a_gapwidth"],
        evaluated.line_gap_width.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_offset,
        buffer,
        shared,
        &["a_offset"],
        evaluated.line_offset.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_width,
        buffer,
        shared,
        &["a_width"],
        evaluated.line_width.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_floor_width,
        buffer,
        shared,
        &["a_floorwidth"],
        evaluated.line_floor_width.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_pattern,
        buffer,
        shared,
        &["a_pattern_to", "a_pattern_from"],
        evaluated.line_pattern.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.line_dasharray,
        buffer,
        shared,
        &["a_dasharray"],
        evaluated.line_dasharray.is_constant(),
        pau,
        &mut id,
    );
}

/// A texture of the bucket's dash patterns, for a `line-dasharray` that
/// differs by feature.
fn dash_atlas_texture(context: &mut Context, binders: &LineBinders) -> Option<SharedTexture> {
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
    if let Err(error) = context.upload_texture_data(&texture, &image.data, image.size) {
        Log::error(Event::Render, format!("Uploading the dash patterns failed: {error}"));
        return None;
    }
    Some(texture)
}

impl LayerType for Line {
    type Layout = PatternLayout<LineLayoutSpec>;
    type Bucket = LineBucket;
    type TileBucket<'a> = LayerBucket<'a, Line>;
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

    fn query_radius(bucket: &LineBucket, layer: &EvaluatedLayer<Line>) -> f32 {
        bucket.query_radius(layer)
    }

    fn render_layer(style: Arc<LayerStyle<Line>>) -> Box<dyn RenderLayer> {
        Box::new(RenderLineLayer::new(style))
    }
}

pub(crate) struct RenderLineLayer {
    base: RenderLayerBase,
    values: LayerValues<Line>,
    drawing: TileDraws<LineTile>,
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

    line_shader_group: Option<ShaderGroupPtr>,
    line_gradient_shader_group: Option<ShaderGroupPtr>,
    line_sdf_shader_group: Option<ShaderGroupPtr>,
    line_gradient_sdf_shader_group: Option<ShaderGroupPtr>,
    // Used by the line-pattern branch.
    line_pattern_shader_group: Option<ShaderGroupPtr>,
}

impl RenderLineLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Line>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderLineLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            color_ramp_texture: None,
            color_ramp_changed: false,
            line_shader_group: None,
            line_gradient_shader_group: None,
            line_sdf_shader_group: None,
            line_gradient_sdf_shader_group: None,
            line_pattern_shader_group: None,
        }
    }

    fn update_color_ramp(&mut self) {
        let color_value = self.values.unevaluated.line_gradient.get_value().clone();
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

    // query
    fn line_width(
        &self,
        feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        zoom: f32,
        feature_state: &mln_style::expression::expression::FeatureState,
    ) -> f32 {
        let evaluated = &self.values.evaluated.paint;
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

impl TypedLayer for RenderLineLayer {
    type Type = Line;
    type Drawing = TileDraws<LineTile>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    fn complete_evaluation(
        &mut self,
        parameters: &PropertyEvaluationParameters,
        _evaluated: &mut EvaluatedLayer<Line>,
    ) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = self.values.unevaluated.line_gradient.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp();
        }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Line>) -> RenderPass {
        let evaluated = &evaluated.paint;
        if *evaluated.line_layer_opacity.uniform() > 0.0
            && evaluated.line_opacity.constant_or(&1.0) > 0.0
            && evaluated.line_color.constant_or(&Color::black()).a > 0.0
            && evaluated.line_width.constant_or(&1.0) > 0.0
        {
            RenderPass::Translucent
        } else {
            RenderPass::None
        }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn update_draws(
        &mut self,
        paint_parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Line>],
    ) {
        let shaders = &paint_parameters.static_data.shaders;
        if std::mem::take(&mut self.color_ramp_changed) {
            // The gradients' draws read the old ramp's texture.
            self.remove_all_draws();
        }

        let frame = self.drawing.frame_mut();
        let paint = &self.values.evaluated.paint;
        frame.set_composite(Composite::of(*paint.line_layer_opacity.uniform(), *paint.line_layer_blend.uniform()));
        frame.set_stencil_tiles(render_tiles.iter().map(|tile| tile.tile.id));

        let render_pass = RenderPass(self.values.evaluated.render_passes & !RenderPass::Opaque.underlying_type());
        for tile in render_tiles {
            let tile_id = tile.id();

            let Some(render_data) = tile.bucket_for_pass(render_pass) else {
                self.drawing.tiles_mut().drop_value(&tile_id);
                continue;
            };
            let bucket = render_data.bucket;
            if bucket.triangles.is_empty() {
                self.drawing.tiles_mut().drop_value(&tile_id);
                continue;
            }
            let Some(binders) = bucket.paint_property_binders.get(&self.base.style.base().id) else {
                continue;
            };
            let evaluated = &render_data.layer.paint;

            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };
            if entry.value.as_ref().is_some_and(|cached| cached.pass != render_pass) {
                // The tile's draw was built for other passes.
                *entry.value = None;
            }
            // A draw built from another bucket goes.
            entry.set_bucket(bucket.base.bucket_id);

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *paint_parameters.context;
            let cached = if let Some(cached) = entry.value {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    let buffer = &binders.interleaved_vertex_buffer;
                    let paint =
                        paint_vertices(context, render_data.geometry(), buffer, binders.depends_on_feature_state());
                    read_line_attributes(attrs, binders, paint.as_ref(), evaluated, &mut Default::default());
                });
                cached
            } else {
                let built = self.build_tile(shaders, context, render_data, binders, evaluated, render_pass);
                let Some(cached) = built.and_then(|built| self.drawing.tiles_mut().insert(&tile_id, built).ok()) else {
                    continue;
                };
                cached
            };
            if let Some(uniforms) = Self::uniforms(
                &self.values.evaluated,
                tile_id.to_unwrapped(),
                render_data,
                binders,
                cached,
                paint_parameters,
                &atlases.line,
            ) {
                let draw = FrameDraw { pass: cached.pass, clip: true, ..FrameDraw::new(Some(tile_id), cached.id) };
                self.draw_list.push(draw, uniforms);
            }
        }

        let frame = self.drawing.frame_mut();
        let layer = &self.values.evaluated;
        self.draw_list.finish(frame, paint_parameters.context, |writer| Self::write_uniforms(layer, writer));
    }

    /// Adds the dash patterns of the tiles' buckets to the line atlas, then
    /// the layer's own, which the encode draws a dashed tile with when its
    /// bucket has no per-feature patterns.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, Line>]) {
        for tile in tiles {
            let Some(bucket) = tile.bucket() else {
                continue;
            };
            let dasharray = bucket.layer.paint.line_dasharray.faded();
            if dasharray.from.is_empty() {
                continue;
            }
            let cap = dash_pattern_cap(bucket.bucket);
            params.atlases.line.add(dasharray.from, dasharray.to, cap);
        }
        // A dashed draw asks for the layer's pattern even where it is empty.
        let layer_dasharray = self.values.evaluated.paint.line_dasharray.faded();
        for tile in tiles {
            let Some(bucket) = tile.bucket() else {
                continue;
            };
            let laid_out = &bucket.layer.paint.line_dasharray;
            let dashed = !laid_out.is_constant() || !laid_out.faded().from.is_empty();
            let per_feature = bucket
                .bucket
                .paint_property_binders
                .get(&self.base.style.base().id)
                .is_some_and(|binders| binders.line_dasharray.atlas().is_some());
            if dashed && !per_feature {
                let cap = dash_pattern_cap(bucket.bucket);
                params.atlases.line.add(layer_dasharray.from, layer_dasharray.to, cap);
            }
        }
    }
}

impl RenderLayer for RenderLineLayer {
    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        let evaluated = &self.values.evaluated.paint;
        // Translate query geometry
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.line_translate.uniform(),
            *evaluated.line_translate_anchor.uniform(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );
        let query = translated_query_geometry.as_ref().unwrap_or(query_geometry);

        // Evaluate function
        let offset = evaluated.line_offset.evaluate(&EvaluationContext::with_zoom_feature_state(
            zoom,
            Some(feature),
            Some(feature_state),
        )) * pixels_to_tile_units;
        // Test intersection
        let half_width =
            (f64::from(self.line_width(feature, zoom, feature_state)) / 2.0 * f64::from(pixels_to_tile_units)) as f32;

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

impl RenderLineLayer {
    /// The draw of the tile's `bucket`, laid out with `evaluated`, in
    /// `pass`, if the layer has what it draws with.
    fn build_tile(
        &mut self,
        shaders: &ShaderRegistry,
        context: &mut Context,
        tile_bucket: LayerBucket<'_, Line>,
        binders: &LineBinders,
        evaluated: &LinePaintProperties::PossiblyEvaluated,
        pass: RenderPass,
    ) -> Option<LineTile> {
        let (bucket, geometry) = (tile_bucket.bucket, tile_bucket.geometry());
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        let mut properties_as_uniforms = StringIDSetsPair::default();
        let mut attrs = VertexAttributeArray::new();
        read_line_attributes(&mut attrs, binders, paint.as_ref(), evaluated, &mut properties_as_uniforms);

        let has_dasharray =
            !evaluated.line_dasharray.is_constant() || !evaluated.line_dasharray.faded().from.is_empty();
        // A constant dash array's texture comes from the line atlas, with the
        // draw's uniforms.
        let dash_texture =
            if evaluated.line_dasharray.is_constant() { None } else { Some(dash_atlas_texture(context, binders)?) };
        let has_gradient = !self.values.unevaluated.line_gradient.get_value().is_undefined();

        let mut textures = Textures::default();
        let mut atlases = None;
        let (line_type, group, name) = if has_dasharray && has_gradient {
            textures[ID_LINE_IMAGE_TEXTURE] = Some(self.color_ramp_texture(context)?);
            textures[ID_LINE_DASH_TEXTURE] = dash_texture;
            (LineType::GradientSdf, &mut self.line_gradient_sdf_shader_group, "LineGradientSDFShader")
        } else if has_dasharray {
            textures[ID_LINE_IMAGE_TEXTURE] = dash_texture;
            (LineType::Sdf, &mut self.line_sdf_shader_group, "LineSDFShader")
        } else if !self.values.unevaluated.line_pattern.is_undefined() {
            let icons = Some(tile_bucket.atlas_textures()).filter(|atlases| atlases.icon().is_some())?;
            atlases = Some(AtlasBinding::new(
                Some(icons.clone()),
                None,
                Some(ID_LINE_IMAGE_TEXTURE),
                /*is_text*/ false,
                /*sdf_icons*/ true, // to force linear filter
                AlignmentType::Auto,
                /*icon_scaled*/ false,
                /*text_size_is_zoom_constant*/ false,
            ));
            (LineType::Pattern, &mut self.line_pattern_shader_group, "LinePatternShader")
        } else if has_gradient {
            textures[ID_LINE_IMAGE_TEXTURE] = Some(self.color_ramp_texture(context)?);
            (LineType::Gradient, &mut self.line_gradient_shader_group, "LineGradientShader")
        } else {
            (LineType::Simple, &mut self.line_shader_group, "LineShader")
        };
        if group.is_none() {
            *group = shaders.shader_group(name);
        }
        let shader = group.as_ref()?.shader(&properties_as_uniforms, self.base.projection_variant);
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
        let color_mode =
            if pass == RenderPass::Translucent { ColorMode::alpha_blended() } else { ColorMode::unblended() };
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
    /// the tile `tile_id` of `tile_bucket`, whose binders are `binders`, with
    /// the layer's values
    /// `layer`: its matrix, the interpolation of the layer's values, and
    /// where its patterns are. A dashed draw of the line atlas's pattern
    /// takes the atlas's texture here; `None` where it has none yet, and
    /// doesn't draw.
    fn uniforms(
        layer: &EvaluatedLayer<Line>,
        tile_id: UnwrappedTileID,
        tile_bucket: LayerBucket<'_, Line>,
        binders: &LineBinders,
        cached: &mut LineTile,
        parameters: &PaintParameters<'_>,
        line_atlas: &LineAtlas,
    ) -> Option<LineUniforms> {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        let state = parameters.state;
        let zoom = state.get_zoom() as f32;
        let int_zoom = state.get_integer_zoom();
        let pixel_scale = state.get_projection().pixel_scale(state) as f32;

        let projection = projection_data(
            &tile_id,
            parameters,
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
                let texture = cached
                    .objects
                    .texture(ID_LINE_IMAGE_TEXTURE)
                    .and_then(|texture| parameters.context.texture(texture));
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
                        parameters.pixel_ratio,
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
                        sdfgamma: atlas_size.width as f32 / (min_scale * 256.0 * parameters.pixel_ratio) / 2.0,
                    }
                } else {
                    let dasharray = evaluated.line_dasharray.faded();
                    let dash_pattern = line_atlas.get(dasharray.from, dasharray.to, cached.dash_cap)?;
                    cached.objects.texture_or_insert_with(dash_texture_id, || dash_pattern.texture().cloned())?;

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
                        sdfgamma: dash_pattern.size().width as f32 / (min_width * 256.0 * parameters.pixel_ratio) / 2.0,
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
