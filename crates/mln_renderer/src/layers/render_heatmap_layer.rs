// The tiles are drawn (additively, kernel density) into an offscreen
// half-float render target; the layer's draw on the frame colors that
// texture through the color ramp.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use mln_gfx::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_DRAWABLE_UBO, ID_HEATMAP_EVALUATED_PROPS_UBO, ID_HEATMAP_IMAGE_TEXTURE,
    ID_HEATMAP_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_TEXTURE_PROPS_UBO, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
    ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, ShaderRegistry, VertexAttributeArray,
};
use mln_map_types::TransformState;
use mln_style::expression::{Dependency, GlobalStateMap, global_state_refs_intersect};
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle};
use mln_style::layers::Heatmap;
use mln_style::layers::heatmap_layer_properties::{HeatmapPaintProperties, default_heatmap_color};
use mln_style::{PropertyEvaluationParameters, StyleValue as _, TransitionParameters};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;

use super::render_line_layer::apply_color_ramp;
use super::{cast_matrix, refresh_paint_attributes, shared_vertices};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices, released_indexes};
use crate::buckets::heatmap_bucket::{HeatmapBinders, HeatmapBucket, HeatmapLayoutVertex};
use crate::draw::DrawObjects;
use crate::draw_cache::{FrameDraw, FrameDraws, KeptDraw, LayerDraws};
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::layer_tweaker::{bind_no_projection, get_projection_data, globe_extrude_scale, to_projection_ubo};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::heatmap_layout::HeatmapLayout;
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{LayerValues, RenderLayer, RenderLayerBase, TypedLayer};
use crate::render_pass::RenderPass;
use crate::render_source::RenderTiles;
use crate::render_static_data::RenderStaticData;
use crate::render_target::RenderTarget;
use crate::update_parameters::UpdateParameters;

fn read_heatmap_attributes(
    attrs: &mut VertexAttributeArray,
    b: &HeatmapBinders,
    shared: &VertexVectorBasePtr,
    evaluated: &HeatmapPaintProperties::PossiblyEvaluated,
    pau: &mut StringIDSetsPair,
) {
    let buffer = &b.interleaved_vertex_buffer;
    let mut id = ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &b.heatmap_weight,
        buffer,
        shared,
        &["a_weight"],
        evaluated.heatmap_weight.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &b.heatmap_radius,
        buffer,
        shared,
        &["a_radius"],
        evaluated.heatmap_radius.is_constant(),
        pau,
        &mut id,
    );
}

const HEATMAP_SHADER_GROUP_NAME: &str = "HeatmapShader";
const HEATMAP_TEXTURE_SHADER_GROUP_NAME: &str = "HeatmapTextureShader";

/// The pass the layer's draws draw in, onto the frame and into its render
/// target.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// A tile's draw's entries in the render target's storage buffers.
struct HeatmapUniforms {
    drawable: HeatmapDrawableUBO,
    projection: ProjectionUBO,
}

impl LayerType for Heatmap {
    type Layout = HeatmapLayout;
    type Bucket = HeatmapBucket;

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Heatmap>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<HeatmapLayout>, Error> {
        HeatmapLayout::new(parameters.bucket_parameters, group, source_layer, parameters.filter_groups).map(Some)
    }

    fn render_layer(style: Arc<LayerStyle<Heatmap>>) -> Box<dyn RenderLayer> {
        Box::new(RenderHeatmapLayer::new(style))
    }
}

pub(crate) struct RenderHeatmapLayer {
    base: RenderLayerBase,
    values: LayerValues<Heatmap>,
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,
    segments: Vec<SegmentBase>,

    heatmap_shader_group: Option<ShaderGroupPtr>,
    heatmap_texture_shader: Option<ShaderProgramPtr>,
    /// The texture the tiles draw their heat into, which the layer's draw
    /// colors onto the frame.
    render_target: Option<RenderTarget>,
    /// The tiles' draws into the render target, of GPU objects in the tiles'
    /// caches.
    target_draws: FrameDraws,

    shared_texture_vertices: Option<VertexVectorBasePtr>,
}

impl RenderHeatmapLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Heatmap>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderHeatmapLayer {
            base,
            values,
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            segments: Vec::new(),
            heatmap_shader_group: None,
            heatmap_texture_shader: None,
            render_target: None,
            target_draws: FrameDraws::new(),
            shared_texture_vertices: None,
        }
    }

    fn update_color_ramp(&mut self) {
        let mut color_value = self.values.unevaluated.heatmap_color.get_value().clone();
        if color_value.is_undefined() {
            color_value = default_heatmap_color();
        }

        // The ramp image is shared with the texture (C++ shared_ptr); a new
        // one replaces it.
        let mut image = (*self.color_ramp).clone();
        apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref());
        self.color_ramp = Rc::new(image);
    }
}

impl TypedLayer for RenderHeatmapLayer {
    type Type = Heatmap;

    fn parts(&self) -> (&RenderLayerBase, &LayerValues<Heatmap>) {
        (&self.base, &self.values)
    }

    fn parts_mut(&mut self) -> (&mut RenderLayerBase, &mut LayerValues<Heatmap>) {
        (&mut self.base, &mut self.values)
    }
}

impl RenderLayer for RenderHeatmapLayer {
    /// Drops the draws, which were built for the previous style's values.
    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Arc<dyn ErasedLayerStyle>,
        draws: &mut LayerDraws,
    ) {
        self.remove_all_drawables(draws);
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = self.values.unevaluated.heatmap_color.get_value();
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

        let properties = self.values.evaluate(parameters);

        self.base.passes = if *properties.paint.heatmap_opacity.uniform() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        };
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn remove_all_drawables(&mut self, draws: &mut LayerDraws) {
        draws.clear_drawables();
        self.target_draws.draws.clear();
    }

    fn upload_render_targets(&mut self, context: &Context, draws: &mut LayerDraws) {
        if self.base.is_renderable && self.render_target.is_some() {
            draws.upload_draws(&self.target_draws, context);
        }
    }

    fn render_targets(&mut self, parameters: &mut PaintParameters<'_>, draws: &mut LayerDraws) {
        if self.base.is_renderable
            && let Some(render_target) = &mut self.render_target
        {
            let target_draws = &self.target_draws;
            render_target.render(parameters, |gpu, parameters| draws.render_draws(target_draws, gpu, parameters));
        }
    }

    /// Makes the tiles' draws into the render target, which it builds for a
    /// tile it keeps none of, and the layer's draw of the frame, which colors
    /// the render target's heat onto it, and writes the buffers they read.
    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) {
        self.update_projection_variant(state, draws);
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables(draws);
            return;
        };

        let viewport_size = state.get_size();
        let size = Size::new(viewport_size.width / 2, viewport_size.height / 2);
        let render_target = self
            .render_target
            .get_or_insert_with(|| RenderTarget::new(context, size, TextureChannelDataType::HalfFloat));
        render_target.set_size(context, size);
        let render_target_texture = render_target.get_texture();
        if self.draw_tiles(shaders, context, &render_tiles, paint_parameters, draws) {
            self.draw_texture(shaders, context, render_target_texture, paint_parameters, draws);
        }
    }
}

impl RenderHeatmapLayer {
    /// Makes the tiles' draws into the render target, with their GPU objects
    /// in the tiles' caches in `draws`, and writes the buffers they read.
    /// Returns false if the layer draws nothing.
    fn draw_tiles(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        render_tiles: &RenderTiles,
        paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) -> bool {
        if self.heatmap_shader_group.is_none() {
            self.heatmap_shader_group = shaders.shader_group(HEATMAP_SHADER_GROUP_NAME);
        }
        let Some(heatmap_shader_group) = self.heatmap_shader_group.clone() else {
            self.remove_all_drawables(draws);
            return false;
        };
        if (RENDER_PASS.underlying_type() & self.base.evaluated_properties.render_passes()) == 0 {
            return false;
        }

        let evaluated_properties = Arc::clone(&self.values.evaluated);
        let evaluated = &evaluated_properties.paint;
        // The shader of the first tile this builds, which the others take.
        let mut heatmap_shader: Option<ShaderProgramPtr> = None;
        let mut tile_draws: Vec<(FrameDraw, HeatmapUniforms)> = Vec::new();
        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let Some(render_data) = super::render_data_for_pass(tile, &*self.base.style, RENDER_PASS) else {
                draws.drop_cache(&tile_id);
                continue;
            };
            let Some(bucket) = render_data.bucket::<Heatmap>() else {
                continue;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(self.get_id()) else {
                continue;
            };
            let binders = &*binders_lock.read();
            let geometry = tile.get_bucket_geometry();
            let paint = || {
                paint_vertices(geometry, bucket, &binders.interleaved_vertex_buffer, binders.depends_on_feature_state())
            };

            if draws.bucket(&tile_id).is_some_and(|prev| prev != bucket.base.bucket_id) {
                // The tile's draw was built from another bucket.
                draws.drop_cache(&tile_id);
            }
            draws.set_bucket(&tile_id, bucket.base.bucket_id);

            // The draw's attributes point into the bucket's shared vectors,
            // which upload again when they change.
            let changes = draws.take_changes(&tile_id);
            if let Some(cached) = draws.cache_mut::<KeptDraw>(&tile_id) {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    read_heatmap_attributes(attrs, binders, &paint(), evaluated, &mut Default::default());
                });
            } else if !bucket.vertices.is_empty() {
                let mut properties_as_uniforms = StringIDSetsPair::default();
                let mut attrs = VertexAttributeArray::new();
                read_heatmap_attributes(&mut attrs, binders, &paint(), evaluated, &mut properties_as_uniforms);
                let shader = heatmap_shader.get_or_insert_with(|| {
                    heatmap_shader_group.shader(&properties_as_uniforms, self.base.projection_variant)
                });
                if let Some(attr) = attrs.set(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
                    attr.set_shared_raw_data(
                        Some(bucket_vertices(geometry, bucket, &bucket.vertices)),
                        0, // offsetof(HeatmapLayoutVertex, a1)
                        /*vertex_offset=*/ 0,
                        size_of::<HeatmapLayoutVertex>() as u32,
                        AttributeDataType::Short2,
                    );
                }
                let mut objects = DrawObjects::new(String::new());
                objects.set_color_mode(&ColorMode::additive());
                objects.set_cull_face_mode(CullFaceMode::disabled());
                objects.set_shader(Some(Rc::clone(shader)));
                let vertex_count = if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() };
                objects.update_vertex_attributes(
                    Some(Rc::new(RefCell::new(attrs))),
                    vertex_count,
                    DrawMode::Triangles(),
                    Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                    &bucket.segments,
                );
                draws.set_cache(&tile_id, KeptDraw::new(objects));
            }

            let Some(cached) = draws.cache_mut::<KeptDraw>(&tile_id) else {
                continue;
            };
            let draw = FrameDraw { depth: None, ..FrameDraw::new(Some(tile_id), cached.id) };
            tile_draws.push((draw, Self::tile_uniforms(&tile_id, binders, paint_parameters)));
        }

        tile_draws.sort_by_key(|(draw, _)| draw.order());
        if !tile_draws.is_empty() {
            self.write_tile_uniforms(&tile_draws, context);
        }
        self.target_draws.draws = tile_draws.into_iter().map(|(draw, _)| draw).collect();
        true
    }

    /// The entries in the render target's storage buffers of the draw of
    /// `tile_id`, whose bucket's binders are `binders`.
    fn tile_uniforms(
        tile_id: &OverscaledTileID,
        binders: &HeatmapBinders,
        parameters: &PaintParameters<'_>,
    ) -> HeatmapUniforms {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let zoom = parameters.state.get_zoom() as f32;
        let tile_id = tile_id.to_unwrapped();
        let projection = get_projection_data(
            &tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let latitude_scale = parameters.state.get_projection().circle_radius_correction(parameters.state);
        HeatmapUniforms {
            drawable: HeatmapDrawableUBO {
                matrix: cast_matrix(&projection.fallback_matrix),
                extrude_scale: tile_id.pixels_to_tile_units(1.0, zoom),
                weight_t: binders.heatmap_weight.interpolation_factor(zoom),
                radius_t: binders.heatmap_radius.interpolation_factor(zoom),
                globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// Writes the render target's buffers of `draws`, the tiles' in drawing
    /// order.
    fn write_tile_uniforms(&mut self, draws: &[(FrameDraw, HeatmapUniforms)], context: &Context) {
        let evaluated = &self.values.evaluated.paint;
        let props = HeatmapEvaluatedPropsUBO {
            weight: evaluated.heatmap_weight.constant_or(&HeatmapPaintProperties::Defaults::heatmap_weight()),
            radius: evaluated.heatmap_radius.constant_or(&HeatmapPaintProperties::Defaults::heatmap_radius()),
            intensity: *evaluated.heatmap_intensity.uniform(),
            padding: 0.0,
        };
        let buffers = &mut self.target_draws.uniform_buffers;
        buffers.create_or_update_pod(ID_HEATMAP_EVALUATED_PROPS_UBO, &props, context);
        let drawables: Vec<HeatmapDrawableUBO> = draws.iter().map(|(_, uniforms)| uniforms.drawable).collect();
        buffers.write_storage(ID_HEATMAP_DRAWABLE_UBO, bytemuck::cast_slice(&drawables), context);
        let projections: Vec<ProjectionUBO> = draws.iter().map(|(_, uniforms)| uniforms.projection).collect();
        buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projections), context);
    }

    /// Makes the layer's draw of the frame, which colors the heat of
    /// `render_target_texture` onto it, with GPU objects it builds again each
    /// frame in the layer's cache in `draws`, and writes the buffers it
    /// reads.
    fn draw_texture(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        render_target_texture: Texture2DPtr,
        paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) {
        draws.frame.get_or_insert_with(FrameDraws::new);
        if self.heatmap_texture_shader.is_none() {
            self.heatmap_texture_shader =
                shaders.shader(HEATMAP_TEXTURE_SHADER_GROUP_NAME, ProjectionVariant::Mercator);
        }
        let Some(heatmap_texture_shader) = self.heatmap_texture_shader.clone() else {
            self.remove_all_drawables(draws);
            return;
        };

        let vertices = Rc::clone(
            self.shared_texture_vertices
                .get_or_insert_with(|| shared_vertices(&RenderStaticData::heatmap_texture_vertices())),
        );
        let mut attrs = VertexAttributeArray::new();
        if let Some(attr) = attrs.set(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices),
                0, // offsetof(HeatmapLayoutVertex, a1)
                /*vertex_offset=*/ 0,
                size_of::<HeatmapLayoutVertex>() as u32,
                AttributeDataType::Short2,
            );
        }
        if self.segments.is_empty() {
            self.segments = RenderStaticData::heatmap_texture_segments();
        }

        let color_ramp = context.create_texture_2d();
        {
            let mut texture = color_ramp.borrow_mut();
            texture.set_image(Some(Rc::clone(&self.color_ramp)));
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }

        let mut objects = DrawObjects::new(String::new());
        objects.set_color_mode(&ColorMode::alpha_blended());
        objects.set_cull_face_mode(CullFaceMode::disabled());
        objects.set_shader(Some(heatmap_texture_shader));
        objects.set_texture(Some(render_target_texture), ID_HEATMAP_IMAGE_TEXTURE);
        objects.set_texture(Some(color_ramp), ID_HEATMAP_COLOR_RAMP_TEXTURE);
        objects.update_vertex_attributes(
            Some(Rc::new(RefCell::new(attrs))),
            4,
            DrawMode::Triangles(),
            Some(released_indexes(&RenderStaticData::quad_triangle_indices())),
            &self.segments,
        );
        let draw = KeptDraw::new(objects);
        let frame_draw = FrameDraw { depth: None, ..FrameDraw::new(None, draw.id) };
        draws.set_layer_cache(draw);

        let Some(frame) = &mut draws.frame else {
            return;
        };
        frame.draws = vec![frame_draw];
        let mut m: Mat4 = [0.0; 16];
        let size = paint_parameters.renderable_size;
        matrix::ortho(&mut m, 0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);
        let props = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *self.values.evaluated.paint.heatmap_opacity.uniform(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        frame.uniform_buffers.create_or_update_pod(ID_HEATMAP_TEXTURE_PROPS_UBO, &props, context);
        bind_no_projection(&mut frame.uniform_buffers, context);
    }
}
