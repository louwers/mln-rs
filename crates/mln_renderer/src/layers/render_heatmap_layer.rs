// The tiles are drawn (additively, kernel density) into an offscreen
// half-float render target; a second, non-tile layer group draws that
// texture through the color ramp onto the map.

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
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
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::heatmap_layer_tweaker::HeatmapLayerTweaker;
use super::heatmap_texture_layer_tweaker::HeatmapTextureLayerTweaker;
use super::render_line_layer::apply_color_ramp;
use super::{DrawableBinders, DrawableRenderTile, refresh_if_bucket_updated, shared_vertices, update_tile};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices, released_indexes};
use crate::buckets::heatmap_bucket::{HeatmapBinders, HeatmapBucket, HeatmapLayoutVertex};
use crate::drawable::Drawable;
use crate::drawable_builder::DrawableBuilder;
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweaker;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType, evaluated};
use crate::layout::heatmap_layout::HeatmapLayout;
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{RenderLayer, RenderLayerBase, set_style};
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
    style: Arc<LayerStyle<Heatmap>>,
    // Paint properties
    unevaluated: HeatmapPaintProperties::Unevaluated,
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,
    segments: Vec<SegmentBase>,

    heatmap_shader_group: Option<ShaderGroupPtr>,
    heatmap_texture_shader: Option<ShaderProgramPtr>,
    /// The texture the tiles draw their heat into, which the layer's group
    /// colors onto the frame.
    render_target: Option<RenderTarget>,

    shared_texture_vertices: Option<VertexVectorBasePtr>,

    /// This is the layer tweaker for applying the off-screen texture to the
    /// framebuffer. The inherited layer tweaker is for applying tiles to the
    /// off-screen texture.
    texture_tweaker: Option<Box<dyn LayerTweaker>>,
}

impl RenderHeatmapLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Heatmap>>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn ErasedEvaluatedLayer> = Arc::new(EvaluatedLayer::<Heatmap>::new(Arc::clone(&impl_)));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderHeatmapLayer {
            base,
            style: impl_,
            unevaluated,
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            segments: Vec::new(),
            heatmap_shader_group: None,
            heatmap_texture_shader: None,
            render_target: None,
            shared_texture_vertices: None,
            texture_tweaker: None,
        }
    }

    fn update_color_ramp(&mut self) {
        let mut color_value = self.unevaluated.heatmap_color.get_value().clone();
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

impl RenderLayer for RenderHeatmapLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        set_style(&mut self.base, &mut self.style, style);
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = self.style.paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn layer_changed(&mut self, _parameters: &TransitionParameters, _new_layer: &Arc<dyn ErasedLayerStyle>) {
        // RenderLayer::layerChanged
        self.base.layer_tweaker = None;
        self.texture_tweaker = None;
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
        let ramp_value = self.unevaluated.heatmap_color.get_value();
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

        let previous_evaluated = evaluated::<Heatmap>(&self.base.evaluated_properties).paint.clone();
        let mut properties = EvaluatedLayer::<Heatmap>::with_paint(
            Arc::clone(&self.style),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes = if *properties.paint.heatmap_opacity.uniform() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        };
        properties.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &mut self.base.layer_tweaker {
            layer_tweaker.update_properties(Arc::clone(&self.base.evaluated_properties));
        }
        if let Some(texture_tweaker) = &mut self.texture_tweaker {
            texture_tweaker.update_properties(Arc::clone(&self.base.evaluated_properties));
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn remove_all_drawables(&mut self) {
        // RenderLayer::removeAllDrawables
        if let Some(group) = &mut self.base.layer_group {
            group.clear_drawables();
        }
        if let Some(render_target) = &mut self.render_target {
            render_target.layer_group.clear_drawables();
        }
    }

    /// The texture tweaker sets up the layer's group, which colors the heat
    /// onto the frame; the layer's tweaker sets up the render target's.
    fn tweak(&mut self, parameters: &PaintParameters<'_>) {
        if self.base.is_renderable
            && let (Some(tweaker), Some(group)) = (&mut self.texture_tweaker, &mut self.base.layer_group)
        {
            tweaker.execute(group, parameters);
        }
    }

    fn upload_render_targets(&mut self, context: &Context) {
        if self.base.is_renderable
            && let Some(render_target) = &mut self.render_target
        {
            render_target.upload(context);
        }
    }

    fn render_targets(&mut self, parameters: &mut PaintParameters<'_>) {
        if self.base.is_renderable
            && let Some(render_target) = &mut self.render_target
        {
            let tweaker = &mut self.base.layer_tweaker;
            render_target.render(parameters, |group, parameters| {
                if let Some(tweaker) = tweaker {
                    tweaker.execute(group, parameters);
                }
            });
        }
    }

    fn tile_layer_groups<'a>(&'a mut self, groups: &mut Vec<&'a mut LayerGroup>) {
        groups.extend(self.base.layer_group.as_mut());
        groups.extend(self.render_target.as_mut().map(|render_target| &mut render_target.layer_group));
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
    ) {
        self.update_projection_variant(state);
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };

        let viewport_size = state.get_size();
        let size = Size::new(viewport_size.width / 2, viewport_size.height / 2);

        // Set up a render target, with its tile layer group
        let mut render_target = self.render_target.take().unwrap_or_else(|| {
            self.texture_tweaker = None;
            RenderTarget::new(context, size, TextureChannelDataType::HalfFloat, LayerGroup::tiles())
        });
        render_target.set_size(context, size);
        let drawn = self.draw_tiles(shaders, &render_tiles, &mut render_target.layer_group);
        let render_target_texture = render_target.get_texture();
        self.render_target = Some(render_target);
        if drawn {
            self.draw_texture(shaders, context, render_target_texture);
        }
    }
}

impl RenderHeatmapLayer {
    /// Updates the drawables of the tiles, which draw into the render target
    /// through `tile_layer_group`. Returns false if the layer draws nothing.
    fn draw_tiles(
        &mut self,
        shaders: &ShaderRegistry,
        render_tiles: &RenderTiles,
        tile_layer_group: &mut LayerGroup,
    ) -> bool {
        if self.heatmap_shader_group.is_none() {
            self.heatmap_shader_group = shaders.shader_group(HEATMAP_SHADER_GROUP_NAME);
        }
        let Some(heatmap_shader_group) = self.heatmap_shader_group.clone() else {
            tile_layer_group.clear_drawables();
            if let Some(group) = &mut self.base.layer_group {
                group.clear_drawables();
            }
            return false;
        };

        if self.base.layer_tweaker.is_none() {
            self.base.layer_tweaker = Some(Box::new(HeatmapLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
        }
        let tweaker = self.base.layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);

        let render_pass = RenderPass::Translucent;

        if (render_pass.underlying_type() & self.base.evaluated_properties.render_passes()) == 0 {
            return false;
        }

        {
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                drawable.tile_id.is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }

        let evaluated_properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = &evaluated::<Heatmap>(&evaluated_properties).paint;
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();

        let mut heatmap_shader: Option<ShaderProgramPtr> = None;

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let style = Arc::clone(&self.base.style);
            let Some(render_data) = super::render_data_for_pass(tile, &*style, render_pass) else {
                tile_layer_group.remove_drawables(render_pass, &tile_id);
                continue;
            };

            let Some(bucket) = (*render_data.bucket).downcast_ref::<HeatmapBucket>() else {
                continue;
            };
            let vertex_count = bucket.vertices.len();
            let Some(binders_lock) = bucket.paint_property_binders.get(self.get_id()) else {
                continue;
            };
            let binders_guard = binders_lock.read();
            let paint_property_binders = &*binders_guard;
            let geometry = tile.get_bucket_geometry();

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id.is_some_and(|prev| prev != bucket.base.bucket_id) {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                tile_layer_group.remove_drawables(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            let paint = || {
                paint_vertices(
                    geometry,
                    bucket,
                    &paint_property_binders.interleaved_vertex_buffer,
                    paint_property_binders.depends_on_feature_state(),
                )
            };
            let update_existing = |drawable: &mut Drawable| {
                // A drawable produced on a previous style/bucket should not
                // be updated.
                let current = drawable.tweaker == tweaker;
                if current {
                    refresh_if_bucket_updated(drawable, |attrs| {
                        read_heatmap_attributes(
                            attrs,
                            paint_property_binders,
                            &paint(),
                            evaluated,
                            &mut Default::default(),
                        );
                    });
                }
                current
            };
            if update_tile(tile_layer_group, render_pass, &tile_id, update_existing) {
                continue;
            }

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            let heatmap_vertex_attrs = VertexAttributeArray::new_ptr();
            read_heatmap_attributes(
                &mut heatmap_vertex_attrs.borrow_mut(),
                paint_property_binders,
                &paint(),
                evaluated,
                &mut properties_as_uniforms,
            );

            if heatmap_shader.is_none() {
                heatmap_shader =
                    Some(heatmap_shader_group.shader(&properties_as_uniforms, self.base.projection_variant));
            }

            if let Some(attr) = heatmap_vertex_attrs.borrow_mut().set(
                ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
                -1,
                AttributeDataType::Invalid,
                0,
            ) {
                attr.set_shared_raw_data(
                    Some(bucket_vertices(geometry, bucket, &bucket.vertices)),
                    0, // offsetof(HeatmapLayoutVertex, a1)
                    /*vertex_offset=*/ 0,
                    size_of::<HeatmapLayoutVertex>() as u32,
                    AttributeDataType::Short2,
                );
            }

            let mut heatmap_builder = DrawableBuilder::boxed();
            heatmap_builder.set_shader(heatmap_shader.clone());
            heatmap_builder.set_enable_depth(false);
            heatmap_builder.set_color_mode(&ColorMode::additive());
            heatmap_builder.set_cull_face_mode(CullFaceMode::disabled());
            heatmap_builder.set_render_pass(render_pass);
            heatmap_builder.set_vertex_attributes(Some(heatmap_vertex_attrs));
            heatmap_builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
            heatmap_builder.set_segments_shared(
                DrawMode::Triangles(),
                Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                &bucket.segments,
            );

            heatmap_builder.flush();

            for mut drawable in heatmap_builder.clear_drawables() {
                drawable.tile_id = Some(tile_id);
                drawable.tweaker = tweaker;
                drawable.binders = Some(DrawableBinders::new(&render_data.bucket, self.get_id()));
                drawable.render_tile = DrawableRenderTile::new(render_tiles.owner(), tile);
                tile_layer_group.add_tile_drawable(render_pass, &tile_id, drawable);
            }
        }
        true
    }

    /// Rebuilds the drawable of the layer's group, which colors the heat of
    /// `render_target_texture` onto the frame.
    fn draw_texture(&mut self, shaders: &ShaderRegistry, context: &Context, render_target_texture: Texture2DPtr) {
        let render_pass = RenderPass::Translucent;

        // Set up texture layer group
        let layer_group = self.base.layer_group.get_or_insert_with(LayerGroup::plain);
        if self.texture_tweaker.is_none() {
            self.texture_tweaker = Some(Box::new(HeatmapTextureLayerTweaker::new(
                self.base.style.base().id.clone(),
                Arc::clone(&self.base.evaluated_properties),
            )));
        }
        let texture_tweaker = self.texture_tweaker.as_ref().map(|tweaker| tweaker.base().key);

        if self.heatmap_texture_shader.is_none() {
            self.heatmap_texture_shader =
                shaders.shader(HEATMAP_TEXTURE_SHADER_GROUP_NAME, ProjectionVariant::Mercator);
        }
        let Some(heatmap_texture_shader) = self.heatmap_texture_shader.clone() else {
            layer_group.clear_drawables();
            if let Some(render_target) = &mut self.render_target {
                render_target.layer_group.clear_drawables();
            }
            return;
        };

        // TODO: Don't rebuild drawables every time
        layer_group.clear_drawables();

        if self.shared_texture_vertices.is_none() {
            self.shared_texture_vertices = Some(shared_vertices(&RenderStaticData::heatmap_texture_vertices()));
        }
        let shared_texture_vertices = self.shared_texture_vertices.clone().expect("texture vertices");
        let texture_vertex_count = 4;

        let texture_vertex_attrs = VertexAttributeArray::new_ptr();
        if let Some(attr) =
            texture_vertex_attrs.borrow_mut().set(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
        {
            attr.set_shared_raw_data(
                Some(shared_texture_vertices),
                0, // offsetof(HeatmapLayoutVertex, a1)
                /*vertex_offset=*/ 0,
                size_of::<HeatmapLayoutVertex>() as u32,
                AttributeDataType::Short2,
            );
        }

        let mut heatmap_texture_builder = DrawableBuilder::boxed();
        heatmap_texture_builder.set_shader(Some(heatmap_texture_shader));
        heatmap_texture_builder.set_enable_depth(false);
        heatmap_texture_builder.set_color_mode(&ColorMode::alpha_blended());
        heatmap_texture_builder.set_cull_face_mode(CullFaceMode::disabled());
        heatmap_texture_builder.set_render_pass(render_pass);
        heatmap_texture_builder.set_vertex_attributes(Some(texture_vertex_attrs));
        heatmap_texture_builder.set_raw_vertices(Vec::new(), texture_vertex_count, AttributeDataType::Short2);
        if self.segments.is_empty() {
            self.segments = RenderStaticData::heatmap_texture_segments();
        }
        heatmap_texture_builder.set_segments_shared(
            DrawMode::Triangles(),
            Some(released_indexes(&RenderStaticData::quad_triangle_indices())),
            &self.segments,
        );

        heatmap_texture_builder.set_texture(Some(render_target_texture), ID_HEATMAP_IMAGE_TEXTURE);

        let texture = context.create_texture_2d();
        {
            let mut t = texture.borrow_mut();
            t.set_image(Some(Rc::clone(&self.color_ramp)));
            t.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        heatmap_texture_builder.set_texture(Some(texture), ID_HEATMAP_COLOR_RAMP_TEXTURE);

        heatmap_texture_builder.flush();

        for mut drawable in heatmap_texture_builder.clear_drawables() {
            drawable.tweaker = texture_tweaker;
            layer_group.add_drawable(drawable);
        }
    }
}
