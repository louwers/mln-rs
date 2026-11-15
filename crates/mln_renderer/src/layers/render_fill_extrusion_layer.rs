// Each tile draws its roofs from the bucket's triangles and its walls as
// instances of one static quad, an instance per bucket vertex. The WebGPU
// and Vulkan walls also read the next vertex as attributes of their own
// (`ID_FILL_EXTRUSION_NEXT_*`), from the same buffer one vertex further on,
// so a tile draws an instance for every vertex but the last, which ends a
// ring and has no wall.
//
// A device that rasterizes on the CPU runs its vertex pipeline once per
// instance, so there each wall is a quad of its own in the vertex buffer:
// every outline vertex is in it four times, once with each corner of the
// quad (`FillExtrusionWallVertex`), and a wall reads the next outline vertex
// from the same buffer four vertices further on. The wall shaders run the
// instanced ones' code with every attribute per vertex: the same vertices in
// the same order, drawn without instances.

use std::cell::RefCell;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_IMAGE_TEXTURE,
    ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::fill_extrusion_layer_impl::Impl as FillExtrusionLayerImpl;
use mln_style::layers::fill_extrusion_layer_properties::{FillExtrusionLayerProperties, FillExtrusionPaintProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::AlignmentType;
use mln_util::hash::HashSet;

use super::fill_extrusion_layer_tweaker::FillExtrusionLayerTweaker;
use super::{DrawableBinders, DrawableRenderTile, TileDrawables, drawable_has_layer_tweaker, shared_vertices};
use crate::bucket::Bucket;
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, derived_vertices, paint_vertices};
use crate::buckets::fill_extrusion_bucket::{
    FillExtrusionBinders, FillExtrusionBucket, FillExtrusionLayoutVertex, FillExtrusionStaticVertex,
    FillExtrusionWallVertex,
};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::drawable_builder::{DrawableBuilder, UniqueDrawableBuilder};
use crate::gfx::context::Context;
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{CaptureView, RenderLayer, RenderLayerBase, Stats, vertex_color_alpha};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &FillExtrusionLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), FillExtrusionLayerImpl::static_type_info()));
    impl_.downcast_ref::<FillExtrusionLayerImpl>().expect("RenderFillExtrusionLayer: not a fill-extrusion layer impl")
}

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
    if *evaluated.fill_extrusion_opacity.uniform() == 0.0 {
        return;
    }

    let color = &evaluated.fill_extrusion_color;
    let color_is_constant = color.is_constant();
    if color_is_constant && binders.fill_extrusion_color.uniform_value(color).a == 0.0 {
        return;
    }

    let translation = evaluated.fill_extrusion_translate.uniform();
    let translation_anchor = &evaluated.fill_extrusion_translate_anchor;
    let base = f64::from(
        evaluated.fill_extrusion_base.constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_base()),
    );
    let height = f64::from(
        evaluated.fill_extrusion_height.constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_height()),
    );
    let zoom_fraction = state.get_zoom_fraction();
    let buffer = &binders.interleaved_vertex_buffer;
    let vertices = bucket.vertices.data();
    let mut view: Option<CaptureView<'_>> = None;

    let features = bucket.get_retained_features();
    stats.rendered_features.reserve(features.len());

    for feature in features {
        debug_assert_ne!(feature.feature_id, "");

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
                *translation,
                *translation_anchor.uniform(),
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
            stats.add_rendered_feature(&feature.feature_id, &bound, HashSet::from_iter([*tile_id]));
        }
    }
}

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
        evaluated.fill_extrusion_base.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.fill_extrusion_color.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_height,
        buffer,
        shared,
        &["a_height"],
        evaluated.fill_extrusion_height.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_pattern,
        buffer,
        shared,
        &["a_pattern_to", "a_pattern_from"],
        evaluated.fill_extrusion_pattern.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
}

/// The quads of the walls of `vertices`: each vertex with each corner of the
/// static quad, in its order. The quad of the wall from a vertex is its four
/// corners, and the next four are the next vertex's.
fn wall_quad_vertices(vertices: &[FillExtrusionLayoutVertex]) -> VertexVectorBasePtr {
    let corners = RenderStaticData::fill_extrusion_vertices();
    let quads: Vec<FillExtrusionWallVertex> = vertices
        .iter()
        .flat_map(|&vertex| corners.iter().map(move |&corner| FillExtrusionWallVertex { corner, vertex }))
        .collect();
    Rc::new(VertexVector::from_vec_released(quads))
}

/// The paint values of the quads of `wall_quad_vertices`: those of each of
/// the first `walls` vertices in `paint`, `stride` bytes each, for each
/// corner of its wall.
fn wall_quad_paint(paint: &[u8], stride: usize, walls: usize) -> VertexVectorBasePtr {
    let mut values = Vec::with_capacity(4 * stride * walls);
    if stride > 0 {
        for vertex in paint.chunks_exact(stride).take(walls) {
            for _ in 0..4 {
                values.extend_from_slice(vertex);
            }
        }
    }
    Rc::new(VertexVector::from_vec_released(values))
}

/// Points each attribute of `attrs` that reads `paint` at `quad_paint`
/// (`wall_quad_paint`), which has the same layout. The binders apply their
/// attributes only to a vector of the bucket's vertices.
fn read_quad_paint(attrs: &mut VertexAttributeArray, paint: &VertexVectorBasePtr, quad_paint: &VertexVectorBasePtr) {
    for id in 0..attrs.allocated_size() {
        let Some(attr) =
            attrs.get(id).filter(|attr| attr.get_shared_raw_data().is_some_and(|data| Rc::ptr_eq(data, paint)))
        else {
            continue;
        };
        let (offset, vertex_offset) = (attr.get_shared_offset(), attr.get_shared_vertex_offset());
        let (stride, type_) = (attr.get_shared_stride(), attr.get_shared_type());
        if let Some(attr) = attrs.set(id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(quad_paint)), offset, vertex_offset, stride, type_);
        }
    }
}

/// Reads each wall vertex's corner and its wall's outline vertex from
/// `quads` (`wall_quad_vertices`), and the next outline vertex from the same
/// buffer four vertices further on.
fn read_wall_quads(attrs: &mut VertexAttributeArray, quads: &VertexVectorBasePtr) {
    let stride = size_of::<FillExtrusionWallVertex>() as u32;
    let corner = offset_of!(FillExtrusionWallVertex, corner) as u32;
    let pos = offset_of!(FillExtrusionWallVertex, vertex) as u32;
    let decimals_ed = pos + offset_of!(FillExtrusionLayoutVertex, a2) as u32;
    for (id, offset, vertex_offset, type_) in [
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, corner, 0, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, pos, 0, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, decimals_ed, 0, AttributeDataType::UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, pos, 4, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, decimals_ed, 4, AttributeDataType::UShort2),
    ] {
        if let Some(attr) = attrs.set(id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(quads)), offset, vertex_offset, stride, type_);
        }
    }
}

/// Reads each vertex's corner of the static quad from `corners`, and each
/// instance's vertex and the next one from `vertices`, the bucket's.
fn read_wall_instances(
    attrs: &mut VertexAttributeArray,
    instance_attrs: &mut VertexAttributeArray,
    corners: &VertexVectorBasePtr,
    vertices: &VertexVectorBasePtr,
) {
    let static_stride = size_of::<FillExtrusionStaticVertex>() as u32;
    if let Some(attr) = attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(corners)), 0, 0, static_stride, AttributeDataType::Short2);
    }
    let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
    // An instance's outline vertex, then the next one.
    for (vertex_offset, pos_id, decimals_ed_id) in [
        (0, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        (1, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
    ] {
        if let Some(attr) = instance_attrs.set(pos_id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(vertices)), 0, vertex_offset, stride, AttributeDataType::Short2);
        }
        if let Some(attr) = instance_attrs.set(decimals_ed_id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(vertices)), 4, vertex_offset, stride, AttributeDataType::UShort2);
        }
    }
}

/// The builder of a tile's depth pass or color pass drawables.
fn create_builder(
    shader: &mln_gfx::shaders::shader_program::ShaderProgramPtr,
    enable_color: bool,
) -> UniqueDrawableBuilder {
    let mut builder = DrawableBuilder::boxed();
    builder.set_shader(Some(Rc::clone(shader)));
    builder.set_is_3d(true);
    if enable_color {
        builder.set_color_mode(&ColorMode::alpha_blended());
    }
    builder.set_render_pass(RenderPass::Translucent);
    builder.set_cull_face_mode(CullFaceMode::back_ccw());
    builder.set_draw_priority(i64::from(enable_color));
    builder
}

pub(crate) struct RenderFillExtrusionLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: FillExtrusionPaintProperties::Unevaluated,

    fill_extrusion_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_group: Option<ShaderGroupPtr>,
    /// The walls' shaders: the instanced ones, or the wall shaders where the
    /// walls are quads of their own.
    fill_extrusion_wall_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_wall_group: Option<ShaderGroupPtr>,

    /// The wall quad (`RenderStaticData::fill_extrusion_vertices`) of the
    /// instanced walls.
    static_data_vertices: Option<VertexVectorBasePtr>,
    /// The triangles of the wall quad, or of the walls' own quads
    /// (`RenderStaticData::fill_extrusion_wall_indices`).
    static_data_indices: Option<IndexVectorBasePtr>,
}

impl RenderFillExtrusionLayer {
    pub(crate) fn new(impl_: Arc<FillExtrusionLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(FillExtrusionLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderFillExtrusionLayer {
            base,
            unevaluated,
            fill_extrusion_group: None,
            fill_extrusion_pattern_group: None,
            fill_extrusion_wall_group: None,
            fill_extrusion_pattern_wall_group: None,
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
            downcast_arc::<FillExtrusionLayerImpl>(&self.base.base_impl),
            parameters.get_crossfade_parameters(),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes = if *properties.evaluated.fill_extrusion_opacity.uniform() > 0.0 {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(Arc::clone(&self.base.evaluated_properties));
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
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        _zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&self.base.evaluated_properties);
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(query_geometry),
            geometry,
        )
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
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
            self.set_layer_group(Some(LayerGroup::tiles(self.base.layer_index)), changes);
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(FillExtrusionLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
            self.base.layer_tweaker = Some(Rc::clone(&tweaker));
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(Rc::clone(&layer_group), Rc::downgrade(&tweaker)));
        }
        let layer_tweaker = self.base.layer_tweaker.clone();

        if self.fill_extrusion_group.is_none() {
            self.fill_extrusion_group = shaders.shader_group("FillExtrusionShader");
        }
        if self.fill_extrusion_pattern_group.is_none() {
            self.fill_extrusion_pattern_group = shaders.shader_group("FillExtrusionPatternShader");
        }
        // A device that rasterizes on the CPU draws each wall as a quad of
        // its own.
        let instanced_walls = !context.caps().cpu_rasterizer;
        if self.fill_extrusion_wall_group.is_none() {
            let name = if instanced_walls { "FillExtrusionInstancedShader" } else { "FillExtrusionWallShader" };
            self.fill_extrusion_wall_group = shaders.shader_group(name);
        }
        if self.fill_extrusion_pattern_wall_group.is_none() {
            let name =
                if instanced_walls { "FillExtrusionPatternInstancedShader" } else { "FillExtrusionPatternWallShader" };
            self.fill_extrusion_pattern_wall_group = shaders.shader_group(name);
        }

        let evaluated_properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&evaluated_properties);

        let draw_pass = RenderPass::Translucent;

        {
            let mut tile_layer_group = layer_group.borrow_mut();
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                // If the render pass has changed or the tile has dropped out
                // of the cover set, remove it.
                let tile_id = drawable.tile_id;
                !drawable.has_render_pass(draw_pass)
                    || tile_id.is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }

        let has_pattern = !self.unevaluated.fill_extrusion_pattern.is_undefined();
        let opaque = *evaluated.fill_extrusion_opacity.uniform() >= 1.0;

        let mut depth_builder: Option<UniqueDrawableBuilder> = None;
        let mut color_builder: Option<UniqueDrawableBuilder> = None;
        let mut wall_depth_builder: Option<UniqueDrawableBuilder> = None;
        let mut wall_color_builder: Option<UniqueDrawableBuilder> = None;

        let (shader_group, wall_shader_group) = if has_pattern {
            (self.fill_extrusion_pattern_group.clone(), self.fill_extrusion_pattern_wall_group.clone())
        } else {
            (self.fill_extrusion_group.clone(), self.fill_extrusion_wall_group.clone())
        };
        let (Some(shader_group), Some(wall_shader_group)) = (shader_group, wall_shader_group) else {
            self.remove_all_drawables();
            return;
        };

        {
            let mut tile_layer_group = layer_group.borrow_mut();
            tile_layer_group.stencil_tiles = Some(Rc::new(render_tiles.iter().map(|tile| tile.id).collect()));
        }

        let static_vertices = Rc::clone(
            self.static_data_vertices
                .get_or_insert_with(|| shared_vertices(&RenderStaticData::fill_extrusion_vertices())),
        );
        let static_indices = Rc::clone(self.static_data_indices.get_or_insert_with(|| {
            Rc::new(RefCell::new(if instanced_walls {
                IndexVectorBase::new(RenderStaticData::fill_extrusion_triangle_indices())
            } else {
                IndexVectorBase::new_released(RenderStaticData::fill_extrusion_wall_indices())
            }))
        }));
        let instance_vertex_count = static_vertices.get_raw_count();

        let mut properties_as_uniforms: StringIDSetsPair = Default::default();
        let mut wall_properties_as_uniforms: StringIDSetsPair = Default::default();
        let layer_id = self.get_id().to_owned();
        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = Arc::clone(&self.base.base_impl);
            let render_data = super::render_data_for_pass(tile, &*base_impl, draw_pass);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.remove_tile(draw_pass, &tile_id);
                continue;
            };

            let Some(bucket) = (*render_data.bucket).downcast_ref::<FillExtrusionBucket>() else {
                continue;
            };

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id.is_some_and(|prev| prev != bucket.base.bucket_id) {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(draw_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            for builder in [&mut depth_builder, &mut color_builder, &mut wall_depth_builder, &mut wall_color_builder]
                .into_iter()
                .flatten()
            {
                builder.set_atlases(None);
            }

            let vertex_count = bucket.vertices.len();
            let Some(binders_lock) = bucket.paint_property_binders.get(&layer_id) else {
                continue;
            };
            let binders_guard = binders_lock.read();
            let binders = &*binders_guard;
            let geometry = tile.get_bucket_geometry();

            // Capture rendered features if requested
            if update_parameters.capture_rendered_features {
                let params = &render_tree.get_parameters().transform_params;
                capture_rendered_features(&mut self.base.stats, bucket, tile, binders, evaluated, state, params);
            }

            // If we already have drawables for this tile, update them.
            let update_existing = |drawable: &mut crate::drawable::Drawable| {
                // A drawable produced on a previous style/bucket should not
                // be updated.
                drawable_has_layer_tweaker(drawable, layer_tweaker.as_ref())
            };
            if self.update_tile(draw_pass, &tile_id, update_existing) {
                continue;
            }

            // A wall from each vertex to the next, but from the last, which
            // ends a ring.
            let walls = vertex_count.saturating_sub(1);
            let buffer = &binders.interleaved_vertex_buffer;
            let depends_on_feature_state = binders.depends_on_feature_state();
            // The walls' own quads are made of the bucket's vertices and paint
            // values before those move to the vectors the roofs draw.
            let quads = (!instanced_walls).then(|| {
                let vertices = derived_vertices(geometry, bucket, &bucket.vertices, wall_quad_vertices);
                let paint_of = |paint: &[u8]| wall_quad_paint(paint, buffer.stride, walls);
                let paint = if depends_on_feature_state {
                    paint_of(&buffer.shared_vertex_vector.data())
                } else {
                    derived_vertices(geometry, bucket, &buffer.shared_vertex_vector, paint_of)
                };
                (vertices, paint)
            });

            // The roofs read the paint values per vertex, the instanced walls
            // per instance, from the same buffer.
            let paint = paint_vertices(geometry, bucket, buffer, depends_on_feature_state);
            let vertices = bucket_vertices(geometry, bucket, &bucket.vertices);
            let vertex_attrs = VertexAttributeArray::new_ptr();
            read_paint_properties(
                &mut vertex_attrs.borrow_mut(),
                binders,
                evaluated,
                &paint,
                &mut properties_as_uniforms,
            );
            let wall_attrs = VertexAttributeArray::new_ptr();
            let (wall_instance_attrs, wall_vertex_count, wall_segments) =
                if let Some((quad_vertices, quad_paint)) = &quads {
                    let mut attrs = wall_attrs.borrow_mut();
                    read_paint_properties(&mut attrs, binders, evaluated, &paint, &mut wall_properties_as_uniforms);
                    read_quad_paint(&mut attrs, &paint, quad_paint);
                    read_wall_quads(&mut attrs, quad_vertices);
                    (None, 4 * walls, RenderStaticData::fill_extrusion_wall_segments(walls))
                } else {
                    let instance_attrs = VertexAttributeArray::new_ptr();
                    let mut attrs = instance_attrs.borrow_mut();
                    read_paint_properties(&mut attrs, binders, evaluated, &paint, &mut wall_properties_as_uniforms);
                    read_wall_instances(&mut wall_attrs.borrow_mut(), &mut attrs, &static_vertices, &vertices);
                    drop(attrs);
                    let segments = vec![RenderStaticData::fill_extrusion_segment(walls)];
                    (Some(instance_attrs), instance_vertex_count, segments)
                };

            let variant = self.base.projection_variant;
            let shader = shader_group.shader(&properties_as_uniforms, variant);
            let wall_shader = wall_shader_group.shader(&wall_properties_as_uniforms, variant);

            // The non-pattern path in `render()` only uses two-pass
            // rendering if there's translucency. The pattern path always
            // uses two passes.
            let do_depth_pass = !opaque || has_pattern;

            if do_depth_pass && depth_builder.is_none() {
                depth_builder = Some(create_builder(&shader, false));
            }
            if color_builder.is_none() {
                color_builder = Some(create_builder(&shader, true));
            }
            if do_depth_pass && wall_depth_builder.is_none() {
                wall_depth_builder = Some(create_builder(&wall_shader, false));
            }
            if wall_color_builder.is_none() {
                wall_color_builder = Some(create_builder(&wall_shader, true));
            }

            if has_pattern && let Some(atlases) = tile.get_atlas_textures() {
                let atlases = Rc::new(DrawableAtlasesTweaker::new(
                    Some(Rc::clone(atlases)),
                    None,
                    Some(ID_FILL_EXTRUSION_IMAGE_TEXTURE),
                    /*is_text=*/ false,
                    false,
                    AlignmentType::Auto,
                    false,
                    false,
                ));
                for builder in
                    [&mut depth_builder, &mut color_builder, &mut wall_depth_builder, &mut wall_color_builder]
                        .into_iter()
                        .flatten()
                {
                    builder.set_atlases(Some(Rc::clone(&atlases)));
                }
            }

            let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
            {
                let mut attrs = vertex_attrs.borrow_mut();
                if let Some(attr) = attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
                {
                    // offsetof(FillExtrusionLayoutVertex, a1)
                    attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 0, 0, stride, AttributeDataType::Short2);
                }
                if let Some(attr) =
                    attrs.set(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
                {
                    // offsetof(FillExtrusionLayoutVertex, a2)
                    attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 4, 0, stride, AttributeDataType::UShort2);
                }
            }

            if do_depth_pass && let Some(builder) = &mut depth_builder {
                builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(Rc::clone(&vertex_attrs)));
            }

            if let Some(builder) = &mut color_builder {
                builder.set_enable_stencil(do_depth_pass);
                builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(vertex_attrs));
            }

            if do_depth_pass && let Some(builder) = &mut wall_depth_builder {
                builder.set_raw_vertices(Vec::new(), wall_vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(Rc::clone(&wall_attrs)));
                builder.set_instance_attributes(wall_instance_attrs.clone());
            }

            if let Some(builder) = &mut wall_color_builder {
                builder.set_enable_stencil(do_depth_pass);
                builder.set_raw_vertices(Vec::new(), wall_vertex_count, AttributeDataType::Short2);
                builder.set_vertex_attributes(Some(wall_attrs));
                builder.set_instance_attributes(wall_instance_attrs);
            }

            let triangles = bucket_indexes(geometry, bucket, &bucket.triangles);
            let finish = |builder: &mut DrawableBuilder, indexes: &IndexVectorBasePtr, segments: &[SegmentBase]| {
                builder.set_segments_shared(DrawMode::Triangles(), Some(Rc::clone(indexes)), segments);

                builder.flush();

                let mut tile_layer_group = layer_group.borrow_mut();
                for mut drawable in builder.clear_drawables() {
                    drawable.tile_id = Some(tile_id);
                    drawable.type_ = usize::from(has_pattern);
                    drawable.layer_tweaker = layer_tweaker.clone();
                    drawable.binders = Some(DrawableBinders::new(&render_data.bucket, &layer_id));
                    drawable.render_tile = DrawableRenderTile::new(render_tiles.owner(), tile);

                    tile_layer_group.add_tile_drawable(draw_pass, &tile_id, drawable);
                }
            };
            let (depth, wall_depth) =
                if do_depth_pass { (depth_builder.as_mut(), wall_depth_builder.as_mut()) } else { (None, None) };
            if !bucket.triangles.is_empty() {
                for builder in [depth, color_builder.as_mut()].into_iter().flatten() {
                    finish(builder.as_mut(), &triangles, &bucket.triangle_segments);
                }
            }
            // A tile with one vertex has no wall.
            if walls > 0 {
                for builder in [wall_depth, wall_color_builder.as_mut()].into_iter().flatten() {
                    finish(builder.as_mut(), &static_indices, &wall_segments);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;
    use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;

    use super::*;

    // A wall's quad is the static quad's corners, each with the wall's
    // vertex, and the next four are the next vertex's. Its paint values are
    // its instance's, in each corner.
    #[test]
    fn wall_quads_are_the_instances() {
        let vertices: Vec<FillExtrusionLayoutVertex> = (0..4)
            .map(|i| FillExtrusionBucket::layout_vertex(Point::new(f64::from(i) + 0.5, 1.0), i as u16, i == 3))
            .collect();
        let quads = wall_quad_vertices(&vertices);
        let quads = quads.get_raw_data();
        let quads: &[FillExtrusionWallVertex] = bytemuck::cast_slice(&quads);
        assert_eq!(quads.len(), 4 * 4);
        let corners = RenderStaticData::fill_extrusion_vertices();
        for (vertex, quad) in vertices.iter().zip(quads.chunks(4)) {
            for (&corner, wall_vertex) in corners.iter().zip(quad) {
                assert_eq!(*wall_vertex, FillExtrusionWallVertex { corner, vertex: *vertex });
            }
        }

        // Four vertices of three bytes: the last one's starts no wall.
        let paint: Vec<u8> = (0..12).collect();
        let quad_paint = wall_quad_paint(&paint, 3, 3);
        let expected: Vec<u8> = paint[..9].chunks(3).flat_map(|values| values.repeat(4)).collect();
        assert_eq!(*quad_paint.get_raw_data(), *expected);
        assert_eq!(wall_quad_paint(&[], 0, 3).get_raw_count(), 0);
    }

    // The segments draw every wall, each from the start of the indices, which
    // address no more vertices than 16 bits do.
    #[test]
    fn wall_segments_draw_every_wall() {
        let indices = RenderStaticData::fill_extrusion_wall_indices();
        assert_eq!(indices[..12], [0, 2, 1, 1, 2, 3, 4, 6, 5, 5, 6, 7]);
        let most = indices.len() / 6;
        assert_eq!(usize::from(*indices.iter().max().unwrap()), 4 * most - 1);
        assert!(4 * most <= MAX_SEGMENT_VERTICES);

        assert_eq!(RenderStaticData::fill_extrusion_wall_segments(0).len(), 0);
        let segments = RenderStaticData::fill_extrusion_wall_segments(most + 2);
        let lengths: Vec<_> = segments
            .iter()
            .map(|s| (s.vertex_offset, s.index_offset, s.vertex_length, s.index_length, s.instance_count))
            .collect();
        assert_eq!(lengths, [(0, 0, 4 * most, 6 * most, 1), (4 * most, 0, 8, 12, 1)]);
    }
}
