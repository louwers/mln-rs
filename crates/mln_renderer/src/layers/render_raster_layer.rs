// Differences in mechanics, not in output:
// - C++ keeps the GPU texture in the (shared) RasterBucket; here the layer
//   keeps one texture per bucket id (`textures`), because buckets are Send +
//   Sync and textures are render-thread `Rc`s.
// - C++ re-applies the static vertex attributes to existing tile drawables
//   every frame (`buildVertexData(none, &drawable, bucket)`); the port only
//   checks, as C++ does there, whether the bucket's masked geometry changed
//   since the drawable was created, and rebuilds the tile if so. The static
//   geometry never changes, so the drawables are the same.
//

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::texture2d::{SamplerState, Texture2DPtr};
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::vertex_attribute::VertexAttributeArrayPtr;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, Context, CullFaceMode, DepthMaskType, DrawMode, DrawableBuilder, LayerGroupBasePtr,
    ShaderProgramBasePtr, ShaderRegistry,
};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::raster_layer_impl::Impl as RasterLayerImpl;
use mln_style::layers::raster_layer_properties::{
    RasterLayerProperties, RasterOpacity, RasterPaintProperties, RasterResampling, Resampling,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::{RasterResamplingType, ResamplingType};
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;

use super::raster_layer_tweaker::RasterLayerTweaker;
use super::{TileDrawables, drawable_has_layer_tweaker, shared_vertices};
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerPrepareParameters, RenderLayer, RenderLayerBase, activate_layer_group, change_layer_index,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &RasterLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), RasterLayerImpl::static_type_info()));
    impl_.downcast_ref::<RasterLayerImpl>().expect("RenderRasterLayer: not a raster layer impl")
}

pub struct RenderRasterLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: RasterPaintProperties::Unevaluated,

    raster_shader: Option<ShaderProgramBasePtr>,

    static_data_vertices: Option<VertexVectorBasePtr>,
    static_data_indices: Option<IndexVectorBasePtr>,
    static_data_segments: Option<Rc<Vec<SegmentBase>>>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// `bucket.texture2d`, per bucket.
    textures: HashMap<SimpleIdentity, Texture2DPtr>,

    image_data: Option<Rc<ImageSourceRenderData>>,
    image_layer_group: Option<LayerGroupBasePtr>,
}

impl RenderRasterLayer {
    pub fn new(impl_: Arc<RasterLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(RasterLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderRasterLayer {
            base,
            unevaluated,
            raster_shader: None,
            static_data_vertices: None,
            static_data_indices: None,
            static_data_segments: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::new(),
            image_data: None,
            image_layer_group: None,
        }
    }

    /// Creates the bucket's texture if needed and attaches it to both image
    /// slots.
    fn set_textures(&mut self, context: &dyn Context, builder: &mut dyn DrawableBuilder, bucket: &RasterBucket) {
        let Some(image) = &bucket.image else {
            return;
        };
        let texture = self
            .textures
            .entry(bucket.base.bucket_id)
            .or_insert_with(|| {
                let texture = context.create_texture_2d();
                texture.borrow_mut().set_image(Some(Rc::new((**image).clone())));
                texture
            })
            .clone();

        texture.borrow_mut().set_sampler_configuration(self.sampler_state());

        builder.set_texture(Some(texture.clone()), ID_RASTER_IMAGE0_TEXTURE);
        builder.set_texture(Some(texture), ID_RASTER_IMAGE1_TEXTURE);
    }

    /// The sampler of the layer's tile textures for the evaluated
    /// properties.
    fn sampler_state(&self) -> SamplerState {
        let evaluated = get_evaluated::<RasterLayerProperties>(&self.base.evaluated_properties);
        // GL JS draws Nearest when either property asks for it.
        let nearest = *evaluated.get::<RasterResampling>() == RasterResamplingType::Nearest
            || *evaluated.get::<Resampling>() == ResamplingType::Nearest;
        SamplerState {
            filter: if nearest { TextureFilterType::Nearest } else { TextureFilterType::Linear },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        }
    }
}

impl RenderRasterLayer {
    /// The `imageData` branch of `update`: one drawable per tile the image
    /// is drawn in, rebuilt every frame (C++: "TODO: Can we avoid rebuilding
    /// drawables each time in this case as well?").
    fn update_image(
        &mut self,
        context: &dyn Context,
        raster_shader: Option<&ShaderProgramBasePtr>,
        image_data: &ImageSourceRenderData,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if let Some(group) = &self.image_layer_group {
            group.borrow_mut().clear_drawables();
        }

        // C++ dereferences the bucket unconditionally; it is missing until
        // the image loads.
        let Some(bucket) = image_data.bucket.clone() else {
            return;
        };
        let has_vertices = !bucket.geometry().vertices.is_empty();
        if !has_vertices {
            return;
        }

        let layer_tweaker = self.base.layer_tweaker.clone();
        if self.image_layer_group.is_none() {
            // Set up a layer group
            let Some(group) = context.create_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) else {
                return;
            };
            if let Some(tweaker) = &layer_tweaker {
                changes.push(ChangeRequest::AddLayerTweaker(group.clone(), Rc::downgrade(tweaker)));
            }
            self.image_layer_group = Some(group);
            activate_layer_group(&self.image_layer_group, self.base.is_renderable, changes);
        }
        let image_layer_group = self.image_layer_group.clone().expect("image layer group");

        // Create a drawable for each tile the image is drawn in
        // TODO: Share textures
        for tile_id in &image_data.tile_ids {
            let mut builder = context.create_drawable_builder("raster".to_owned());
            builder.set_shader(raster_shader.cloned());
            builder.set_render_pass(RenderPass::Translucent);
            builder.set_sub_layer_index(0);
            builder.set_depth_type(DepthMaskType::ReadOnly);
            builder.set_color_mode(&ColorMode::alpha_blended());
            builder.set_cull_face_mode(&CullFaceMode::disabled());

            {
                // buildVertexData(builder, nullptr, bucket): the bucket's own
                // geometry (shared: vertices, indices and segments are set).
                let geometry = bucket.geometry();
                let vertices = shared_vertices(&geometry.vertices);
                builder.set_vertex_attributes(Some(raster_vertex_attributes(context, &vertices)));
                builder.set_raw_vertices(Vec::new(), geometry.vertices.len(), AttributeDataType::Short2);
                builder.set_segments_shared(
                    DrawMode::Triangles(),
                    Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                    &geometry.segments,
                );
            }
            self.set_textures(context, builder.as_mut(), &bucket);

            // finish
            builder.flush(context);

            let mut group = image_layer_group.borrow_mut();
            let Some(layer_group) = group.as_layer_group_mut() else {
                continue;
            };
            for mut drawable in builder.clear_drawables() {
                drawable.set_tile_id(OverscaledTileID::new(tile_id.canonical.z, tile_id.wrap, tile_id.canonical));
                drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                layer_group.add_drawable(drawable);
            }
        }
    }
}

/// The vertex attributes of a raster drawable over `vertices`.
fn raster_vertex_attributes(context: &dyn Context, vertices: &VertexVectorBasePtr) -> VertexAttributeArrayPtr {
    let vertex_attrs = context.create_vertex_attribute_array();
    {
        let mut attrs = vertex_attrs.borrow_mut();
        let stride = std::mem::size_of::<RasterLayoutVertex>() as u32;
        if let Some(attr) = attrs.set(ID_RASTER_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                0, // offsetof(RasterLayoutVertex, a1)
                /*vertex_offset=*/ 0,
                stride,
                AttributeDataType::Short2,
            );
        }
        if let Some(attr) = attrs.set(ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                4, // offsetof(RasterLayoutVertex, a2)
                /*vertex_offset=*/ 0,
                stride,
                AttributeDataType::Short2,
            );
        }
    }
    vertex_attrs
}

impl RenderLayer for RenderRasterLayer {
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
        let previous_evaluated = get_evaluated::<RasterLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = RasterLayerProperties::with_evaluated(
            downcast_arc::<RasterLayerImpl>(&self.base.base_impl),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes =
            if *properties.evaluated.get::<RasterOpacity>() > 0.0 { RenderPass::Translucent } else { RenderPass::None };
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
        false
    }

    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) {
        // `renderTiles = params.source->getRenderTiles()`; the C++ base
        // prepare is not called (no addRenderPassesFromTiles).
        match params.source {
            Some(source) => {
                self.base.render_tiles = source.get_render_tiles();
                self.base.render_tiles_owner = source.get_raw_render_tiles();
            }
            None => {
                self.base.render_tiles = None;
            }
        }
        self.image_data = params.source.and_then(crate::render_source::RenderSource::get_image_render_data);
        // It is possible image data is not available until the source loads it.
        self.update_render_tile_ids();
    }

    fn mark_layer_renderable(&mut self, will_render: bool, changes: &mut UniqueChangeRequestVec) {
        crate::render_layer::mark_layer_renderable_base(self, will_render, changes);
        if self.image_layer_group.is_some() {
            activate_layer_group(&self.image_layer_group, will_render, changes);
        }
    }

    fn release_render_data(&mut self) {
        crate::render_layer::release_render_data_base(self);
        if let Some(layer_group) = &self.image_layer_group {
            layer_group.borrow_mut().clear_drawables();
        }
        self.textures.clear();
        self.image_data = None;
    }

    fn layer_removed(&mut self, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::layerRemoved
        self.remove_all_drawables();
        let group = self.base.layer_group.clone();
        activate_layer_group(&group, false, changes);
        if self.image_layer_group.is_some() {
            activate_layer_group(&self.image_layer_group, false, changes);
        }
    }

    fn layer_index_changed(&mut self, new_layer_index: i32, changes: &mut UniqueChangeRequestVec) {
        // RenderLayer::layerIndexChanged
        self.base.layer_index = new_layer_index;
        let group = self.base.layer_group.clone();
        change_layer_index(&group, new_layer_index, changes);

        change_layer_index(&self.image_layer_group, new_layer_index, changes);
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if self.update_projection_variant(state) {
            self.raster_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let render_tiles = self.base.render_tiles.clone().filter(|t| !t.is_empty());
        let image_data = self.image_data.clone();
        if render_tiles.is_none() && image_data.is_none() {
            if let Some(layer_group) = &self.base.layer_group {
                layer_group.borrow_mut().clear_drawables();
            }
            if let Some(layer_group) = &self.image_layer_group {
                layer_group.borrow_mut().clear_drawables();
            }
            self.textures.clear();
            return;
        }

        let render_pass = RenderPass::Translucent;

        if self.raster_shader.is_none() {
            self.raster_shader = context.get_generic_shader(shaders, "RasterShader", self.base.projection_variant);
            if self.raster_shader.is_none() {
                return;
            }
        }
        let raster_shader = self.raster_shader.clone();

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(RasterLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            for group in [&self.base.layer_group, &self.image_layer_group].into_iter().flatten() {
                // layerGroup->addLayerTweaker(layerTweaker)
                changes.push(ChangeRequest::AddLayerTweaker(group.clone(), Rc::downgrade(&tweaker)));
            }
        }
        let layer_tweaker = self.base.layer_tweaker.clone();

        if let Some(image_data) = image_data {
            self.update_image(context, raster_shader.as_ref(), &image_data, changes);
            return;
        }
        let Some(render_tiles) = render_tiles else {
            return;
        };

        // Remove existing drawables that are no longer in the cover set
        if let Some(layer_group) = self.base.layer_group.clone() {
            let mut group = layer_group.borrow_mut();
            if let Some(tile_layer_group) = group.as_tile_layer_group_mut() {
                let base = &self.base;
                tile_layer_group.remove_drawables_if(|drawable| {
                    drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
                });
            }
        } else {
            // Set up a tile layer group
            if let Some(layer_group) = context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) {
                if let Some(tweaker) = &layer_tweaker {
                    changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(tweaker)));
                }
                self.set_layer_group(Some(layer_group), changes);
            }
        }
        let Some(layer_group) = self.base.layer_group.clone() else {
            return;
        };

        if self.static_data_vertices.is_none() {
            self.static_data_vertices = Some(shared_vertices(&RenderStaticData::raster_vertices()));
        }
        if self.static_data_indices.is_none() {
            self.static_data_indices =
                Some(Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices()))));
        }
        if self.static_data_segments.is_none() {
            self.static_data_segments = Some(Rc::new(RenderStaticData::raster_segments()));
        }
        let static_vertices = self.static_data_vertices.clone().expect("static vertices");
        let static_indices = self.static_data_indices.clone().expect("static indices");
        let static_segments = self.static_data_segments.clone().expect("static segments");
        let mut static_attrs: Option<VertexAttributeArrayPtr> = None;

        // Drop the textures of buckets that are no longer rendered.
        let live_buckets: std::collections::HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.base_impl).map(|b| b.get_id())).collect();
        self.textures.retain(|id, _| live_buckets.contains(id));
        // Existing drawables keep their textures, so a changed filter is
        // applied to the textures themselves.
        let sampler = self.sampler_state();
        for texture in self.textures.values() {
            texture.borrow_mut().set_sampler_configuration(sampler);
        }

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = self.base.base_impl.clone();
            let bucket_ptr = tile.get_bucket(&*base_impl).filter(|b| b.has_data()).cloned();
            let Some(bucket_ptr) = bucket_ptr else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };
            let Some(bucket) = (*bucket_ptr).downcast_ref::<RasterBucket>() else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };

            let mut cleared = false;
            let (geometry_nonempty, geometry_modified) = {
                let geometry = bucket.geometry();
                (
                    !geometry.vertices.is_empty() && !geometry.indices.is_empty() && !geometry.segments.is_empty(),
                    geometry.last_modified,
                )
            };

            if self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id) {
                // Bucket ID changed, we need to rebuild the drawables
                self.remove_tile(render_pass, &tile_id);
                cleared = true;
            } else if geometry_nonempty {
                // If the bucket data has changed, rebuild the drawables.
                // Find the earliest time on existing drawables
                let mut tile_update_time: Option<Instant> = None;
                {
                    let mut group = layer_group.borrow_mut();
                    if let Some(tile_layer_group) = group.as_tile_layer_group_mut() {
                        tile_layer_group.visit_drawables_for(render_pass, &tile_id, |drawable| {
                            let create_time = drawable.base().create_time;
                            if tile_update_time.is_none_or(|t| create_time < t) {
                                tile_update_time = Some(create_time);
                            }
                        });
                    }
                }

                if tile_update_time.is_some_and(|t| t < geometry_modified) {
                    self.remove_tile(render_pass, &tile_id);
                    cleared = true;
                }
            }

            if !cleared {
                // Update existing drawables
                let mut geometry_changed = false;
                let update_existing = |drawable: &mut dyn mln_gfx::Drawable| {
                    // Only current drawables are updated, ones produced for
                    // a previous style retain the attribute values for that
                    // style.
                    if !drawable_has_layer_tweaker(drawable, &layer_tweaker) {
                        return false;
                    }
                    // buildVertexData(none, &drawable, bucket): the bucket
                    // may later add, remove, or change masking. In that
                    // case, the tile's shared data and segments are not
                    // updated, and it needs to be re-created.
                    if !geometry_changed && drawable.base().create_time < geometry_modified {
                        // Masking changed, need to rebuild this tile
                        geometry_changed = true;
                    }
                    true
                };
                // If we update existing drawables, don't build new ones. But
                // if the geometry has changed, we need to drop and re-build
                // them anyway.
                if self.update_tile(render_pass, &tile_id, update_existing) && !geometry_changed {
                    continue;
                } else if geometry_changed {
                    self.remove_tile(render_pass, &tile_id);
                }
            }

            // Otherwise, create new ones.
            let mut builder = context.create_drawable_builder("raster".to_owned());
            builder.set_shader(raster_shader.clone());
            builder.set_render_pass(render_pass);
            builder.set_sub_layer_index(0);
            builder.set_depth_type(DepthMaskType::ReadOnly);
            builder.set_color_mode(&ColorMode::alpha_blended());
            builder.set_cull_face_mode(&CullFaceMode::disabled());

            self.set_textures(context, builder.as_mut(), bucket);

            // buildVertexData(builder, nullptr, bucket, tileID): the bucket
            // only fills in geometry for masked tiles, otherwise the standard
            // tile extent geometry is used; on the globe, the pole-capped
            // grid of what the mask keeps.
            {
                let geometry = bucket.geometry();
                let globe = self.base.projection_variant == ProjectionVariant::Globe;
                let shared = !globe
                    && !geometry.vertices.is_empty()
                    && !geometry.indices.is_empty()
                    && !geometry.segments.is_empty();
                if globe {
                    let globe_mesh =
                        self.globe_meshes.get(&tile_id.canonical, &geometry.mask, RasterBucket::layout_vertex);
                    let vertices: VertexVectorBasePtr = globe_mesh.vertices.clone();
                    let attrs = raster_vertex_attributes(context, &vertices);
                    builder.set_vertex_attributes(Some(attrs));
                    builder.set_raw_vertices(Vec::new(), globe_mesh.vertices.elements(), AttributeDataType::Short2);
                    builder.set_segments_shared(
                        DrawMode::Triangles(),
                        Some(globe_mesh.indices.clone()),
                        &globe_mesh.segments,
                    );
                } else if shared {
                    let vertices = shared_vertices(&geometry.vertices);
                    let attrs = raster_vertex_attributes(context, &vertices);
                    builder.set_vertex_attributes(Some(attrs));
                    builder.set_raw_vertices(Vec::new(), geometry.vertices.len(), AttributeDataType::Short2);
                    builder.set_segments_shared(
                        DrawMode::Triangles(),
                        Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                        &geometry.segments,
                    );
                } else {
                    let attrs =
                        static_attrs.get_or_insert_with(|| raster_vertex_attributes(context, &static_vertices)).clone();
                    builder.set_vertex_attributes(Some(attrs));
                    builder.set_raw_vertices(Vec::new(), 4, AttributeDataType::Short2);
                    builder.set_segments_shared(DrawMode::Triangles(), Some(static_indices.clone()), &static_segments);
                }
            }

            // finish
            builder.flush(context);
            let drawables = builder.clear_drawables();
            let mut group = layer_group.borrow_mut();
            let Some(tile_layer_group) = group.as_tile_layer_group_mut() else {
                continue;
            };
            for mut drawable in drawables {
                drawable.set_tile_id(tile_id);
                drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));
                tile_layer_group.add_drawable(render_pass, &tile_id, drawable);
            }
        }
    }
}
