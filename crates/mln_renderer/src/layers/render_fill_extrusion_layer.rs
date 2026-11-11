// Ports src/mln/renderer/layers/render_fill_extrusion_layer.hpp and
// src/mln/renderer/layers/render_fill_extrusion_layer.cpp (drawable path,
// `MLN_USE_FILL_EXTRUSION_INSTANCING`).
//
// Each tile draws its roofs from the bucket's triangles and its walls as
// instances of one static quad, an instance per bucket vertex. The WebGPU
// walls also read the next vertex as attributes of their own
// (`ID_FILL_EXTRUSION_NEXT_*`), from the same buffer one vertex further on,
// so a tile draws an instance for every vertex but the last, which ends a
// ring and has no wall.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::drawable_tweaker::DrawableTweakerPtr;
use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, Context, CullFaceMode, DrawMode, DrawableBuilder, ShaderRegistry,
    UniqueDrawableBuilder,
};
use mln_map_types::TransformState;
use mln_style::layer::static_immutable_cast;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::fill_extrusion_layer_impl::Impl as FillExtrusionLayerImpl;
use mln_style::layers::fill_extrusion_layer_properties::*;
use mln_style::properties::Property;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::AlignmentType;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;
use mln_util::sync::RwLockExt;

use super::fill_extrusion_layer_tweaker::FillExtrusionLayerTweaker;
use super::{
    TileDrawables, drawable_binders_slot, drawable_has_layer_tweaker, drawable_render_tile_slot, shared_vertices,
};
use crate::bucket::Bucket;
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::fill_extrusion_bucket::{
    FillExtrusionBinders, FillExtrusionBucket, FillExtrusionLayoutVertex, FillExtrusionStaticVertex,
};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{CaptureView, RenderLayer, RenderLayerBase, Stats, vertex_color_alpha};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Immutable<dyn layer::Impl>) -> &FillExtrusionLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), FillExtrusionLayerImpl::static_type_info()));
    impl_.downcast_ref::<FillExtrusionLayerImpl>().expect("RenderFillExtrusionLayer: not a fill-extrusion layer impl")
}

/// `RenderFillExtrusionLayer::captureRenderedFeatures`.
fn capture_rendered_features(
    stats: &mut Stats,
    bucket: &FillExtrusionBucket,
    tile: &RenderTile,
    binders: &FillExtrusionBinders,
    evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    state: &TransformState,
    transform_params: &TransformParameters,
) {
    let tile_id = tile.get_overscaled_tile_id();

    // fill-extrusion-opacity does not support data-driven expressions
    if *evaluated.get::<FillExtrusionOpacity>() == 0.0 {
        return;
    }

    let color = evaluated.get::<FillExtrusionColor>();
    let color_is_constant = color.is_constant();
    if color_is_constant && binders.fill_extrusion_color.uniform_value(color).a == 0.0 {
        return;
    }

    let translation = evaluated.get::<FillExtrusionTranslate>();
    let translation_anchor = *evaluated.get::<FillExtrusionTranslateAnchor>();
    let base = f64::from(evaluated.get::<FillExtrusionBase>().constant_or(&FillExtrusionBase::default_value()));
    let height = f64::from(evaluated.get::<FillExtrusionHeight>().constant_or(&FillExtrusionHeight::default_value()));
    let zoom_fraction = state.get_zoom_fraction();
    let buffer = &binders.interleaved_vertex_buffer;
    let vertices = bucket.vertices.data();
    let mut view: Option<CaptureView<'_>> = None;

    let features = bucket.get_retained_features();
    stats.rendered_features.reserve(features.len());

    for feature in features {
        debug_assert!(!feature.feature_id.is_empty());

        if !color_is_constant {
            // Consider only the value for the first vertex of this feature, for now.
            let alpha = binders
                .fill_extrusion_color
                .get_vertex_value(buffer, feature.vertex_offset)
                .and_then(|v| vertex_color_alpha(&v, zoom_fraction));
            if alpha == Some(0.0) {
                continue;
            }
        }

        // Compute the view once
        let view = view.get_or_insert_with(|| {
            CaptureView::new(
                tile_id,
                state,
                transform_params,
                translation,
                translation_anchor,
                /*near_clipped=*/ true,
            )
        });

        // Each vertex is considered twice, once at the base z and once at the height z
        let vertex_offset = feature.vertex_offset;
        let vertex_count = feature.vertex_count;
        let get_vertex = |vi: usize| {
            let vertex = vertices.get(vertex_offset.checked_add(vi.checked_rem(vertex_count)?)?)?;
            let z = if vi < vertex_count { base } else { height };
            Some([f64::from(vertex.a1[0]), f64::from(vertex.a1[1]), z])
        };
        if let Some(bound) = view.tile_space_bound(2 * vertex_count, get_vertex) {
            stats.add_rendered_feature(&feature.feature_id, &bound, HashSet::from([*tile_id]));
        }
    }
}

/// `readDataDrivenPaintProperties<FillExtrusionBase, FillExtrusionColor,
/// FillExtrusionHeight, FillExtrusionPattern>(binders, evaluated,
/// propertiesAsUniforms, idFillExtrusionBaseVertexAttribute)`.
fn read_paint_properties(
    attrs: &mut VertexAttributeArray,
    binders: &FillExtrusionBinders,
    evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    shared: &VertexVectorBasePtr,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    properties_as_uniforms.0.clear();
    properties_as_uniforms.1.clear();
    let buffer = &binders.interleaved_vertex_buffer;
    let mut id = ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_base,
        buffer,
        shared,
        &["a_base"],
        evaluated.get::<FillExtrusionBase>().is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.get::<FillExtrusionColor>().is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_height,
        buffer,
        shared,
        &["a_height"],
        evaluated.get::<FillExtrusionHeight>().is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_pattern,
        buffer,
        shared,
        &["a_pattern_to", "a_pattern_from"],
        evaluated.get::<FillExtrusionPattern>().is_constant(),
        properties_as_uniforms,
        &mut id,
    );
}

/// The builder of a tile's depth pass or color pass drawables.
fn create_builder(
    context: &dyn Context,
    name: String,
    shader: &mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr,
    enable_color: bool,
    tweaker: Option<&DrawableTweakerPtr>,
) -> UniqueDrawableBuilder {
    let mut builder = context.create_drawable_builder(name);
    builder.set_shader(Some(shader.clone()));
    builder.set_is_3d(true);
    builder.set_enable_color(enable_color);
    if enable_color {
        builder.set_color_mode(&ColorMode::alpha_blended());
    }
    builder.set_render_pass(RenderPass::Translucent);
    builder.set_cull_face_mode(&CullFaceMode::back_ccw());
    builder.set_draw_priority(if enable_color { 1 } else { 0 });
    if let Some(tweaker) = tweaker {
        builder.add_tweaker(tweaker.clone());
    }
    builder
}

pub struct RenderFillExtrusionLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: FillExtrusionPaintProperties::Unevaluated,

    fill_extrusion_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_group: Option<ShaderGroupPtr>,
    fill_extrusion_instanced_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_instanced_group: Option<ShaderGroupPtr>,

    /// The wall quad (`RenderStaticData::fill_extrusion_vertices`).
    static_data_vertices: Option<VertexVectorBasePtr>,
    static_data_indices: Option<IndexVectorBasePtr>,
}

impl RenderFillExtrusionLayer {
    pub fn new(impl_: Immutable<FillExtrusionLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Immutable<dyn LayerProperties> = Arc::new(FillExtrusionLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderFillExtrusionLayer {
            base,
            unevaluated,
            fill_extrusion_group: None,
            fill_extrusion_pattern_group: None,
            fill_extrusion_instanced_group: None,
            fill_extrusion_pattern_instanced_group: None,
            static_data_vertices: None,
            static_data_indices: None,
        }
    }
}

impl RenderLayer for RenderFillExtrusionLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = get_evaluated::<FillExtrusionLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = FillExtrusionLayerProperties::with_evaluated(
            static_immutable_cast::<FillExtrusionLayerImpl>(&self.base.base_impl),
            parameters.get_crossfade_parameters(),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes = if *properties.evaluated.get::<FillExtrusionOpacity>() > 0.0 {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        get_crossfade::<FillExtrusionLayerProperties>(&self.base.evaluated_properties).t != 1.0
    }

    fn is_3d(&self) -> bool {
        true
    }

    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        _zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&self.base.evaluated_properties);
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            evaluated.get::<FillExtrusionTranslate>(),
            *evaluated.get::<FillExtrusionTranslateAnchor>(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(query_geometry),
            feature.get_geometries(),
        )
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        self.update_projection_variant(state);
        self.base.stats.rendered_features.clear();

        let render_tiles = match self.base.render_tiles.clone() {
            Some(tiles) if !tiles.is_empty() && self.base.passes != RenderPass::None => tiles,
            _ => {
                self.remove_all_drawables();
                return;
            }
        };

        // Set up a layer group
        if self.base.layer_group.is_none() {
            match context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) {
                Some(layer_group) => self.set_layer_group(Some(layer_group), changes),
                None => return,
            }
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(FillExtrusionLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(&tweaker)));
        }
        let layer_tweaker = self.base.layer_tweaker.clone();

        if self.fill_extrusion_group.is_none() {
            self.fill_extrusion_group = shaders.get_shader_group("FillExtrusionShader");
        }
        if self.fill_extrusion_pattern_group.is_none() {
            self.fill_extrusion_pattern_group = shaders.get_shader_group("FillExtrusionPatternShader");
        }
        if self.fill_extrusion_instanced_group.is_none() {
            self.fill_extrusion_instanced_group = shaders.get_shader_group("FillExtrusionInstancedShader");
        }
        if self.fill_extrusion_pattern_instanced_group.is_none() {
            self.fill_extrusion_pattern_instanced_group =
                shaders.get_shader_group("FillExtrusionPatternInstancedShader");
        }

        let evaluated_properties = self.base.evaluated_properties.clone();
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&evaluated_properties);

        let draw_pass = RenderPass::Translucent;

        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
            let base = &self.base;
            let removed = tile_layer_group.remove_drawables_if(|drawable| {
                // If the render pass has changed or the tile has dropped out
                // of the cover set, remove it.
                let tile_id = drawable.get_tile_id();
                !drawable.has_render_pass(draw_pass)
                    || tile_id.is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
            self.base.stats.drawables_removed += removed;
        }

        let layer_prefix = format!("{}/", self.get_id());
        let has_pattern = !self.unevaluated.get::<FillExtrusionPattern>().is_undefined();
        let opaque = *evaluated.get::<FillExtrusionOpacity>() >= 1.0;

        let mut depth_builder: Option<UniqueDrawableBuilder> = None;
        let mut color_builder: Option<UniqueDrawableBuilder> = None;
        let mut instanced_depth_builder: Option<UniqueDrawableBuilder> = None;
        let mut instanced_color_builder: Option<UniqueDrawableBuilder> = None;

        let (shader_group, instanced_shader_group) = if has_pattern {
            (self.fill_extrusion_pattern_group.clone(), self.fill_extrusion_pattern_instanced_group.clone())
        } else {
            (self.fill_extrusion_group.clone(), self.fill_extrusion_instanced_group.clone())
        };
        let (Some(shader_group), Some(instanced_shader_group)) = (shader_group, instanced_shader_group) else {
            self.remove_all_drawables();
            return;
        };

        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
            tile_layer_group.set_stencil_tiles(Some(Rc::new(render_tiles.iter().map(|tile| tile.id).collect())));
        }

        let static_vertices = self
            .static_data_vertices
            .get_or_insert_with(|| shared_vertices(&RenderStaticData::fill_extrusion_vertices()))
            .clone();
        let static_indices = self
            .static_data_indices
            .get_or_insert_with(|| {
                Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::fill_extrusion_triangle_indices())))
            })
            .clone();
        let instance_vertex_count = static_vertices.get_raw_count();

        let mut properties_as_uniforms: StringIDSetsPair = Default::default();
        let mut instance_properties_as_uniforms: StringIDSetsPair = Default::default();
        let layer_id = self.get_id().to_owned();
        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = self.base.base_impl.clone();
            let render_data = super::render_data_for_pass(tile, &*base_impl, draw_pass);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.remove_tile(draw_pass, &tile_id);
                continue;
            };

            let Some(bucket) = (*render_data.bucket).downcast_ref::<FillExtrusionBucket>() else {
                continue;
            };

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id != SimpleIdentity::EMPTY && prev_bucket_id != bucket.base.bucket_id {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(draw_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            let mut tweaker: Option<DrawableTweakerPtr> = None;
            for builder in
                [&mut depth_builder, &mut color_builder, &mut instanced_depth_builder, &mut instanced_color_builder]
                    .into_iter()
                    .flatten()
            {
                builder.clear_tweakers();
            }

            let vertex_count = bucket.vertices.len();
            let Some(binders_lock) = bucket.paint_property_binders.get(&layer_id) else {
                continue;
            };
            let binders_guard = binders_lock.read_unpoisoned();
            let binders = &*binders_guard;
            let geometry = tile.get_bucket_geometry();

            // Capture rendered features if requested
            if update_parameters.capture_rendered_features {
                let params = &render_tree.get_parameters().transform_params;
                capture_rendered_features(&mut self.base.stats, bucket, tile, binders, evaluated, state, params);
            }

            // If we already have drawables for this tile, update them.
            let update_existing = |drawable: &mut dyn mln_gfx::Drawable| {
                // A drawable produced on a previous style/bucket should not
                // be updated.
                drawable_has_layer_tweaker(drawable, &layer_tweaker)
            };
            if self.update_tile(draw_pass, &tile_id, update_existing) {
                continue;
            }

            // The roofs read the paint values per vertex, the walls per
            // instance, from the same buffer.
            let paint = paint_vertices(
                geometry,
                bucket,
                &binders.interleaved_vertex_buffer,
                binders.depends_on_feature_state(),
            );
            let vertex_attrs = context.create_vertex_attribute_array();
            read_paint_properties(
                &mut vertex_attrs.borrow_mut(),
                binders,
                evaluated,
                &paint,
                &mut properties_as_uniforms,
            );
            let instance_attrs = context.create_vertex_attribute_array();
            read_paint_properties(
                &mut instance_attrs.borrow_mut(),
                binders,
                evaluated,
                &paint,
                &mut instance_properties_as_uniforms,
            );

            let variant = self.base.projection_variant;
            let Some(shader) = shader_group.get_or_create_shader(context, &properties_as_uniforms, variant, "a_pos")
            else {
                continue;
            };
            let Some(instanced_shader) = instanced_shader_group.get_or_create_shader(
                context,
                &instance_properties_as_uniforms,
                variant,
                "a_pos",
            ) else {
                continue;
            };

            // The non-pattern path in `render()` only uses two-pass
            // rendering if there's translucency. The pattern path always
            // uses two passes.
            let do_depth_pass = !opaque || has_pattern;

            if do_depth_pass && depth_builder.is_none() {
                depth_builder =
                    Some(create_builder(context, format!("{layer_prefix}depth"), &shader, false, tweaker.as_ref()));
            }
            if color_builder.is_none() {
                color_builder =
                    Some(create_builder(context, format!("{layer_prefix}color"), &shader, true, tweaker.as_ref()));
            }
            if do_depth_pass && instanced_depth_builder.is_none() {
                instanced_depth_builder = Some(create_builder(
                    context,
                    format!("{layer_prefix}depthInstanced"),
                    &instanced_shader,
                    false,
                    tweaker.as_ref(),
                ));
            }
            if instanced_color_builder.is_none() {
                instanced_color_builder = Some(create_builder(
                    context,
                    format!("{layer_prefix}colorInstanced"),
                    &instanced_shader,
                    true,
                    tweaker.as_ref(),
                ));
            }

            if has_pattern && tweaker.is_none() {
                if let Some(atlases) = tile.get_atlas_textures() {
                    let new_tweaker: DrawableTweakerPtr = Rc::new(DrawableAtlasesTweaker::new(
                        Some(atlases.clone()),
                        None,
                        Some(ID_FILL_EXTRUSION_IMAGE_TEXTURE),
                        /*is_text=*/ false,
                        false,
                        AlignmentType::Auto,
                        false,
                        false,
                    ));
                    for builder in [
                        &mut depth_builder,
                        &mut color_builder,
                        &mut instanced_depth_builder,
                        &mut instanced_color_builder,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        builder.add_tweaker(new_tweaker.clone());
                    }
                    tweaker = Some(new_tweaker);
                }
            }
            let _ = &tweaker;

            let vertices = bucket_vertices(geometry, bucket, &bucket.vertices);
            let stride = std::mem::size_of::<FillExtrusionLayoutVertex>() as u32;
            {
                let mut attrs = vertex_attrs.borrow_mut();
                if let Some(attr) = attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
                {
                    // offsetof(FillExtrusionLayoutVertex, a1)
                    attr.set_shared_raw_data(Some(vertices.clone()), 0, 0, stride, AttributeDataType::Short2);
                }
                if let Some(attr) =
                    attrs.set(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
                {
                    // offsetof(FillExtrusionLayoutVertex, a2)
                    attr.set_shared_raw_data(Some(vertices.clone()), 4, 0, stride, AttributeDataType::UShort2);
                }
            }

            if do_depth_pass && let Some(builder) = &mut depth_builder {
                builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(vertex_attrs.clone()));
            }

            if let Some(builder) = &mut color_builder {
                builder.set_enable_stencil(do_depth_pass);
                builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(vertex_attrs));
            }

            let instance_vertex_attrs = context.create_vertex_attribute_array();
            {
                let static_stride = std::mem::size_of::<FillExtrusionStaticVertex>() as u32;
                let mut attrs = instance_vertex_attrs.borrow_mut();
                if let Some(attr) = attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
                {
                    attr.set_shared_raw_data(
                        Some(static_vertices.clone()),
                        0,
                        0,
                        static_stride,
                        AttributeDataType::Short2,
                    );
                }
            }
            {
                let mut attrs = instance_attrs.borrow_mut();
                // An instance's outline vertex, then the next one.
                for (vertex_offset, pos_id, decimals_ed_id) in [
                    (0, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
                    (1, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
                ] {
                    if let Some(attr) = attrs.set(pos_id, -1, AttributeDataType::Invalid, 0) {
                        attr.set_shared_raw_data(
                            Some(vertices.clone()),
                            0,
                            vertex_offset,
                            stride,
                            AttributeDataType::Short2,
                        );
                    }
                    if let Some(attr) = attrs.set(decimals_ed_id, -1, AttributeDataType::Invalid, 0) {
                        attr.set_shared_raw_data(
                            Some(vertices.clone()),
                            4,
                            vertex_offset,
                            stride,
                            AttributeDataType::UShort2,
                        );
                    }
                }
            }

            if do_depth_pass && let Some(builder) = &mut instanced_depth_builder {
                builder.set_raw_vertices(Vec::new(), instance_vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(instance_vertex_attrs.clone()));
                builder.set_instance_attributes(Some(instance_attrs.clone()));
            }

            if let Some(builder) = &mut instanced_color_builder {
                builder.set_enable_stencil(do_depth_pass);
                builder.set_raw_vertices(Vec::new(), instance_vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(instance_vertex_attrs));
                builder.set_instance_attributes(Some(instance_attrs));
            }

            let triangles = bucket_indexes(geometry, bucket, &bucket.triangles);
            let instance_segments = [RenderStaticData::fill_extrusion_segment(vertex_count.saturating_sub(1))];
            let stats = &mut self.base.stats;
            let mut finish =
                |builder: &mut dyn DrawableBuilder, indexes: &IndexVectorBasePtr, segments: &[SegmentBase]| {
                    builder.set_segments_shared(DrawMode::Triangles(), Some(indexes.clone()), segments);

                    builder.flush(context);

                    let mut group = layer_group.borrow_mut();
                    let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
                    for mut drawable in builder.clear_drawables() {
                        drawable.set_tile_id(tile_id);
                        drawable.set_type(usize::from(has_pattern));
                        drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                        drawable.set_binders(drawable_binders_slot(&render_data.bucket, &layer_id));
                        drawable.set_render_tile(drawable_render_tile_slot(render_tiles.owner(), tile));

                        tile_layer_group.add_drawable(draw_pass, &tile_id, drawable);
                        stats.drawables_added += 1;
                    }
                };
            let (depth, instanced_depth) =
                if do_depth_pass { (depth_builder.as_mut(), instanced_depth_builder.as_mut()) } else { (None, None) };
            if !bucket.triangles.is_empty() {
                for builder in [depth, color_builder.as_mut()].into_iter().flatten() {
                    finish(builder.as_mut(), &triangles, &bucket.triangle_segments);
                }
            }
            // A tile with one vertex has no wall.
            if vertex_count > 1 {
                for builder in [instanced_depth, instanced_color_builder.as_mut()].into_iter().flatten() {
                    finish(builder.as_mut(), &static_indices, &instance_segments);
                }
            }
        }
    }
}
