// Ports src/mln/renderer/layers/render_hillshade_layer.hpp and
// src/mln/renderer/layers/render_hillshade_layer.cpp (drawable path).
//
// Each DEM tile is first drawn into its own render target by the prepare
// shader (slopes from the DEM), then the tile drawable shades that texture.
//
// Differences in mechanics, not in output:
// - C++ keeps the prepare render target in the (shared) HillshadeBucket;
//   here the layer keeps it per bucket id (`bucket_render_targets`), with
//   the bucket's prepare generation (see hillshade_bucket.rs).
// - C++ re-applies the vertex attributes to existing tile drawables every
//   frame; the port does so only when the bucket's masked geometry changed
//   since (the drawable's data holds the geometry time it was built from).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::vertex_attribute::VertexAttributeArrayPtr;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, Context, CullFaceMode, DepthMaskType, DrawMode, Drawable, ShaderProgramBasePtr,
    ShaderRegistry,
};
use mln_map_types::TransformState;
use mln_style::layer::static_immutable_cast;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::hillshade_layer_impl::Impl as HillshadeLayerImpl;
use mln_style::layers::hillshade_layer_properties::*;
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_util::constants;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::hillshade_layer_tweaker::HillshadeLayerTweaker;
use super::hillshade_prepare_layer_tweaker::{HillshadePrepareDrawableData, HillshadePrepareLayerTweaker};
use super::render_heatmap_layer::activate_render_target;
use super::{TileDrawables, drawable_has_layer_tweaker, shared_vertices};
use crate::buckets::hillshade_bucket::{HillshadeBucket, HillshadeLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_target::{RenderTarget, RenderTargetPtr};
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Immutable<dyn layer::Impl>) -> &HillshadeLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), HillshadeLayerImpl::static_type_info()));
    impl_.downcast_ref::<HillshadeLayerImpl>().expect("RenderHillshadeLayer: not a hillshade layer impl")
}

const HILLSHADE_PREPARE_SHADER_GROUP_NAME: &str = "HillshadePrepareShader";
const HILLSHADE_SHADER_GROUP_NAME: &str = "HillshadeShader";

/// The data of a hillshade tile drawable: the time of the bucket geometry
/// its vertex attributes were built from.
struct HillshadeDrawableGeometryTime(Instant);

pub struct RenderHillshadeLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: HillshadePaintProperties::Unevaluated,
    maxzoom: u8,

    hillshade_prepare_shader: Option<ShaderProgramBasePtr>,
    hillshade_shader: Option<ShaderProgramBasePtr>,
    activated_render_targets: Vec<RenderTargetPtr>,

    static_data_shared_vertices: Option<VertexVectorBasePtr>,

    prepare_layer_tweaker: Option<LayerTweakerPtr>,

    /// `bucket.renderTarget` with the bucket's prepare generation, per
    /// bucket id.
    bucket_render_targets: HashMap<SimpleIdentity, (RenderTargetPtr, u64)>,
}

impl RenderHillshadeLayer {
    pub fn new(impl_: Immutable<HillshadeLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Immutable<dyn LayerProperties> = Arc::new(HillshadeLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderHillshadeLayer {
            base,
            unevaluated,
            maxzoom: constants::TERRAIN_RGB_MAXZOOM,
            hillshade_prepare_shader: None,
            hillshade_shader: None,
            activated_render_targets: Vec::new(),
            static_data_shared_vertices: None,
            prepare_layer_tweaker: None,
            bucket_render_targets: HashMap::new(),
        }
    }

    /// `addRenderTarget(renderTarget, changes)`.
    fn add_render_target(&mut self, render_target: &RenderTargetPtr, changes: &mut UniqueChangeRequestVec) {
        activate_render_target(&Some(render_target.clone()), true, changes);
        self.activated_render_targets.push(render_target.clone());
    }

    /// `removeRenderTargets(changes)`.
    fn remove_render_targets(&mut self, changes: &mut UniqueChangeRequestVec) {
        for render_target in &self.activated_render_targets {
            activate_render_target(&Some(render_target.clone()), false, changes);
        }
        self.activated_render_targets.clear();
    }
}

/// The vertex attributes of a hillshade (or prepare) drawable over
/// `vertices`.
fn hillshade_vertex_attributes(context: &dyn Context, vertices: &VertexVectorBasePtr) -> VertexAttributeArrayPtr {
    let vertex_attrs = context.create_vertex_attribute_array();
    {
        let mut attrs = vertex_attrs.borrow_mut();
        let stride = std::mem::size_of::<HillshadeLayoutVertex>() as u32;
        if let Some(attr) = attrs.set(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                0, // offsetof(HillshadeLayoutVertex, a1)
                0,
                stride,
                AttributeDataType::Short2,
            );
        }
        if let Some(attr) = attrs.set(ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                4, // offsetof(HillshadeLayoutVertex, a2)
                0,
                stride,
                AttributeDataType::Short2,
            );
        }
    }
    vertex_attrs
}

impl RenderLayer for RenderHillshadeLayer {
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

    fn layer_changed(
        &mut self,
        _parameters: &TransitionParameters,
        _new_layer: &Immutable<dyn layer::Impl>,
        _changes: &mut UniqueChangeRequestVec,
    ) {
        // RenderLayer::layerChanged
        self.base.layer_tweaker = None;
        self.prepare_layer_tweaker = None;
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = get_evaluated::<HillshadeLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = HillshadeLayerProperties::with_evaluated(
            static_immutable_cast::<HillshadeLayerImpl>(&self.base.base_impl),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes = if *properties.evaluated.get::<HillshadeExaggeration>() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);
        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
        if let Some(prepare_layer_tweaker) = &self.prepare_layer_tweaker {
            prepare_layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) {
        match params.source {
            Some(source) => {
                self.base.render_tiles = source.get_render_tiles();
                self.base.render_tiles_owner = source.get_raw_render_tiles();
                self.maxzoom = source.get_max_zoom();
            }
            None => {
                self.base.render_tiles = None;
            }
        }

        self.update_render_tile_ids();
    }

    fn mark_layer_renderable(&mut self, will_render: bool, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::markLayerRenderable
        self.base.is_renderable = will_render;
        let group = self.base.layer_group.clone();
        crate::render_layer::activate_layer_group(&group, will_render, changes);

        self.remove_render_targets(changes);
    }

    fn layer_removed(&mut self, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::layerRemoved
        self.remove_all_drawables();
        let group = self.base.layer_group.clone();
        crate::render_layer::activate_layer_group(&group, false, changes);

        self.remove_render_targets(changes);
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        _state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
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
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(HillshadeLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(&tweaker)));
        }
        let layer_tweaker = self.base.layer_tweaker.clone();

        if self.hillshade_prepare_shader.is_none() {
            self.hillshade_prepare_shader = context.get_generic_shader(shaders, HILLSHADE_PREPARE_SHADER_GROUP_NAME);
        }

        if self.hillshade_shader.is_none() {
            self.hillshade_shader = context.get_generic_shader(shaders, HILLSHADE_SHADER_GROUP_NAME);
        }

        let (Some(hillshade_prepare_shader), Some(hillshade_shader)) =
            (self.hillshade_prepare_shader.clone(), self.hillshade_shader.clone())
        else {
            self.remove_all_drawables();
            return;
        };

        let render_pass = RenderPass::Translucent;
        if (render_pass.underlying_type() & self.base.evaluated_properties.render_passes) == 0 {
            return;
        }

        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("hillshade layer group is a TileLayerGroup");
            let base = &self.base;
            let removed = tile_layer_group.remove_drawables_if(|drawable| {
                drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
            self.base.stats.drawables_removed += removed;
        }

        if self.static_data_shared_vertices.is_none() {
            self.static_data_shared_vertices = Some(shared_vertices(&RenderStaticData::raster_vertices()));
        }
        let static_data_shared_vertices = self.static_data_shared_vertices.clone().expect("static vertices");
        let static_data_indices: IndexVectorBasePtr =
            Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices())));
        let static_data_segments = RenderStaticData::raster_segments();

        let mut hillshade_prepare_vertex_attrs: Option<VertexAttributeArrayPtr> = None;

        // Drop the render targets of buckets that are no longer rendered
        // (C++ releases them with the bucket).
        let live_buckets: HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.base_impl).map(|b| b.get_id())).collect();
        self.bucket_render_targets.retain(|id, _| live_buckets.contains(id));

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = self.base.base_impl.clone();
            let bucket_ptr = tile.get_bucket(&*base_impl).filter(|b| b.has_data()).cloned();
            let Some(bucket_ptr) = bucket_ptr else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };
            let Some(bucket) = (*bucket_ptr).downcast_ref::<HillshadeBucket>() else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };
            let bucket_id = bucket.base.bucket_id;

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id != SimpleIdentity::EMPTY && prev_bucket_id != bucket_id {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket_id);

            let generation = bucket.render_target_generation();
            let render_target_prepared =
                self.bucket_render_targets.get(&bucket_id).is_some_and(|(_, prepared)| *prepared == generation);
            if !render_target_prepared {
                // Set up tile render target
                let (tilesize, stride, encoding, dem_image) = {
                    let dem = bucket.get_dem_data();
                    (dem.dim as u16, dem.stride, dem.encoding, dem.get_image().clone())
                };
                let render_target = RenderTarget::create(
                    context,
                    Size::new(u32::from(tilesize), u32::from(tilesize)),
                    TextureChannelDataType::UnsignedByte,
                );
                self.bucket_render_targets.insert(bucket_id, (render_target.clone(), generation));
                self.add_render_target(&render_target, changes);

                let Some(single_tile_layer_group) =
                    context.create_tile_layer_group(0, /*initial_capacity=*/ 1, self.get_id().to_owned())
                else {
                    return;
                };
                render_target.borrow_mut().add_layer_group(single_tile_layer_group.clone(), /*replace=*/ true);

                if self.prepare_layer_tweaker.is_none() {
                    self.prepare_layer_tweaker = Some(Rc::new(RefCell::new(HillshadePrepareLayerTweaker::new(
                        self.get_id().to_owned(),
                        self.base.evaluated_properties.clone(),
                    ))));
                }
                let prepare_layer_tweaker = self.prepare_layer_tweaker.clone().expect("prepare tweaker");
                // singleTileLayerGroup->addLayerTweaker(prepareLayerTweaker)
                changes.push(ChangeRequest::AddLayerTweaker(
                    single_tile_layer_group.clone(),
                    Rc::downgrade(&prepare_layer_tweaker),
                ));

                let mut hillshade_prepare_builder = context.create_drawable_builder("hillshadePrepare".to_owned());
                hillshade_prepare_builder.set_shader(Some(hillshade_prepare_shader.clone()));
                hillshade_prepare_builder.set_depth_type(DepthMaskType::ReadOnly);
                hillshade_prepare_builder.set_color_mode(&ColorMode::unblended());
                hillshade_prepare_builder.set_cull_face_mode(&CullFaceMode::disabled());
                hillshade_prepare_builder.set_render_pass(render_pass);
                let attrs = hillshade_prepare_vertex_attrs
                    .get_or_insert_with(|| hillshade_vertex_attributes(context, &static_data_shared_vertices))
                    .clone();
                hillshade_prepare_builder.set_vertex_attributes(Some(attrs));
                hillshade_prepare_builder.set_raw_vertices(Vec::new(), 4, AttributeDataType::Short2);
                hillshade_prepare_builder.set_segments_shared(
                    DrawMode::Triangles(),
                    Some(static_data_indices.clone()),
                    &static_data_segments,
                );

                let texture = context.create_texture_2d();
                {
                    let mut t = texture.borrow_mut();
                    t.set_image(Some(Rc::new(dem_image)));
                    // Use Nearest filtering to match GL JS behavior - the
                    // Sobel kernel samples exact pixel values
                    t.set_sampler_configuration(SamplerState {
                        filter: TextureFilterType::Nearest,
                        wrap_u: TextureWrapType::Clamp,
                        wrap_v: TextureWrapType::Clamp,
                        ..Default::default()
                    });
                }
                hillshade_prepare_builder.set_texture(Some(texture), ID_HILLSHADE_IMAGE_TEXTURE);

                hillshade_prepare_builder.flush(context);

                let mut group = single_tile_layer_group.borrow_mut();
                let tile_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
                for mut drawable in hillshade_prepare_builder.clear_drawables() {
                    drawable.set_tile_id(tile_id);
                    drawable.set_layer_tweaker(layer_tweaker_slot(&prepare_layer_tweaker));
                    drawable.set_data(Some(Box::new(HillshadePrepareDrawableData {
                        stride,
                        encoding,
                        maxzoom: self.maxzoom,
                    })));
                    tile_group.add_drawable(render_pass, &tile_id, drawable);
                    self.base.stats.drawables_added += 1;
                }
            }
            let render_target_texture =
                self.bucket_render_targets.get(&bucket_id).expect("render target").0.borrow().get_texture();

            // Set up tile drawable
            let (vertices, vertex_count, indices, segments, geometry_time) = {
                let geometry = bucket.geometry();
                if !geometry.vertices.is_empty() && !geometry.indices.is_empty() && !geometry.segments.is_empty() {
                    let indices: IndexVectorBasePtr =
                        Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())));
                    (
                        shared_vertices(&geometry.vertices),
                        geometry.vertices.len(),
                        indices,
                        geometry.segments.iter().map(SegmentBase::copy).collect::<Vec<SegmentBase>>(),
                        geometry.last_modified,
                    )
                } else {
                    (
                        static_data_shared_vertices.clone(),
                        4,
                        static_data_indices.clone(),
                        static_data_segments.iter().map(SegmentBase::copy).collect::<Vec<SegmentBase>>(),
                        geometry.last_modified,
                    )
                }
            };

            let update_existing = |drawable: &mut dyn Drawable| {
                // Only current drawables are updated, ones produced for a
                // previous style retain the attribute values for that style.
                if !drawable_has_layer_tweaker(drawable, &layer_tweaker) {
                    return false;
                }

                let built_from = drawable
                    .get_data()
                    .and_then(|data| data.downcast_ref::<HillshadeDrawableGeometryTime>())
                    .map(|time| time.0);
                if built_from != Some(geometry_time) {
                    drawable.update_vertex_attributes(
                        Some(hillshade_vertex_attributes(context, &vertices)),
                        vertex_count,
                        DrawMode::Triangles(),
                        Some(indices.clone()),
                        &segments,
                    );
                    drawable.set_data(Some(Box::new(HillshadeDrawableGeometryTime(geometry_time))));
                }
                drawable.set_texture(Some(render_target_texture.clone()), ID_HILLSHADE_IMAGE_TEXTURE);

                true
            };
            if self.update_tile(render_pass, &tile_id, update_existing) {
                continue;
            }

            let mut hillshade_builder = context.create_drawable_builder("hillshade".to_owned());
            hillshade_builder.set_shader(Some(hillshade_shader.clone()));
            hillshade_builder.set_enable_depth(false);
            hillshade_builder.set_color_mode(&ColorMode::alpha_blended());
            hillshade_builder.set_cull_face_mode(&CullFaceMode::disabled());
            hillshade_builder.set_render_pass(render_pass);
            hillshade_builder.set_vertex_attributes(Some(hillshade_vertex_attributes(context, &vertices)));
            hillshade_builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
            hillshade_builder.set_segments_shared(DrawMode::Triangles(), Some(indices.clone()), &segments);
            hillshade_builder.set_texture(Some(render_target_texture.clone()), ID_HILLSHADE_IMAGE_TEXTURE);

            hillshade_builder.flush(context);

            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
            for mut drawable in hillshade_builder.clear_drawables() {
                drawable.set_tile_id(tile_id);
                drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                drawable.set_data(Some(Box::new(HillshadeDrawableGeometryTime(geometry_time))));

                tile_layer_group.add_drawable(render_pass, &tile_id, drawable);
                self.base.stats.drawables_added += 1;
            }
        }
    }
}
