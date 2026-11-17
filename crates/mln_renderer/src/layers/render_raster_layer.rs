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
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::vertex_attribute::VertexAttributeArrayPtr;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, ShaderProgramPtr, ShaderRegistry,
    VertexAttributeArray,
};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Raster;
use mln_style_types::{RasterResamplingType, ResamplingType};
use mln_tile_id::OverscaledTileID;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;

use super::raster_layer_tweaker::RasterLayerTweaker;
use super::{LayerTiles, shared_vertices, update_tile};
use crate::bucket::NoBucket;
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::draw_cache::{DrawGroup, LayerDraws};
use crate::drawable_builder::DrawableBuilder;
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_composite::Composite;
use crate::layer_group::LayerGroup;
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, LayerValues, RenderLayer, RenderLayerBase, TypedLayer};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::update_parameters::UpdateParameters;

impl LayerType for Raster {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<LayerStyle<Raster>>) -> Box<dyn RenderLayer> {
        Box::new(RenderRasterLayer::new(style))
    }
}

pub(crate) struct RenderRasterLayer {
    base: RenderLayerBase,
    values: LayerValues<Raster>,

    raster_shader: Option<ShaderProgramPtr>,

    static_data_vertices: Option<VertexVectorBasePtr>,
    static_data_indices: Option<IndexVectorBasePtr>,
    static_data_segments: Option<Rc<Vec<SegmentBase>>>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// `bucket.texture2d`, per bucket.
    textures: HashMap<SimpleIdentity, Texture2DPtr>,

    image_data: Option<Rc<ImageSourceRenderData>>,
    /// The group of an image source's drawables, which the layer's tweaker
    /// sets up too.
    image_layer_group: Option<LayerGroup>,
}

impl RenderRasterLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Raster>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderRasterLayer {
            base,
            values,
            raster_shader: None,
            static_data_vertices: None,
            static_data_indices: None,
            static_data_segments: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            image_data: None,
            image_layer_group: None,
        }
    }

    /// Creates the bucket's texture if needed and attaches it to both image
    /// slots.
    fn set_textures(&mut self, context: &Context, builder: &mut DrawableBuilder, bucket: &RasterBucket) {
        let Some(image) = &bucket.image else {
            return;
        };
        let texture = Rc::clone(self.textures.entry(bucket.base.bucket_id).or_insert_with(|| {
            let texture = context.create_texture_2d();
            texture.borrow_mut().set_image(Some(Rc::new((**image).clone())));
            texture
        }));

        texture.borrow_mut().set_sampler(self.sampler_state());

        builder.set_texture(Some(Rc::clone(&texture)), ID_RASTER_IMAGE0_TEXTURE);
        builder.set_texture(Some(texture), ID_RASTER_IMAGE1_TEXTURE);
    }

    /// How the layer's drawables composite onto the frame: as one surface,
    /// for a `raster-layer-blend` other than `normal`.
    fn composite(&self) -> Option<Composite> {
        let paint = &self.values.evaluated.paint;
        Composite::of(1.0, *paint.raster_layer_blend.uniform())
    }

    /// The sampler of the layer's tile textures for the evaluated
    /// properties.
    fn sampler_state(&self) -> SamplerState {
        let evaluated = &self.values.evaluated.paint;
        // GL JS draws Nearest when either property asks for it.
        let nearest = *evaluated.raster_resampling.uniform() == RasterResamplingType::Nearest
            || *evaluated.resampling.uniform() == ResamplingType::Nearest;
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
        context: &Context,
        raster_shader: Option<&ShaderProgramPtr>,
        image_data: &ImageSourceRenderData,
    ) {
        if let Some(group) = &mut self.image_layer_group {
            group.clear_drawables();
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

        let tweaker = self.base.layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);
        // Set up a layer group
        let mut layer_group = self.image_layer_group.take().unwrap_or_else(LayerGroup::plain);

        // Create a drawable for each tile the image is drawn in
        // TODO: Share textures
        for tile_id in &image_data.tile_ids {
            let mut builder = DrawableBuilder::boxed();
            builder.set_shader(raster_shader.cloned());
            builder.set_render_pass(RenderPass::Translucent);
            builder.set_sub_layer_index(0);
            builder.set_depth_type(DepthMaskType::ReadOnly);
            builder.set_color_mode(&ColorMode::alpha_blended());
            builder.set_cull_face_mode(CullFaceMode::disabled());

            {
                // buildVertexData(builder, nullptr, bucket): the bucket's own
                // geometry (shared: vertices, indices and segments are set).
                let geometry = bucket.geometry();
                let vertices = shared_vertices(&geometry.vertices);
                builder.set_vertex_attributes(Some(raster_vertex_attributes(&vertices)));
                builder.set_raw_vertices(Vec::new(), geometry.vertices.len(), AttributeDataType::Short2);
                builder.set_segments_shared(
                    DrawMode::Triangles(),
                    Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                    &geometry.segments,
                );
            }
            self.set_textures(context, builder.as_mut(), &bucket);

            // finish
            builder.flush();

            layer_group.composite = self.composite();
            for mut drawable in builder.clear_drawables() {
                drawable.tile_id = Some(OverscaledTileID::new(tile_id.canonical.z, tile_id.wrap, tile_id.canonical));
                drawable.tweaker = tweaker;
                layer_group.add_drawable(drawable);
            }
        }
        self.image_layer_group = Some(layer_group);
    }
}

/// The vertex attributes of a raster drawable over `vertices`.
fn raster_vertex_attributes(vertices: &VertexVectorBasePtr) -> VertexAttributeArrayPtr {
    let vertex_attrs = VertexAttributeArray::new_ptr();
    {
        let mut attrs = vertex_attrs.borrow_mut();
        let stride = size_of::<RasterLayoutVertex>() as u32;
        if let Some(attr) = attrs.set(ID_RASTER_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(Rc::clone(vertices)),
                0, // offsetof(RasterLayoutVertex, a1)
                /*vertex_offset=*/ 0,
                stride,
                AttributeDataType::Short2,
            );
        }
        if let Some(attr) = attrs.set(ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(Rc::clone(vertices)),
                4, // offsetof(RasterLayoutVertex, a2)
                /*vertex_offset=*/ 0,
                stride,
                AttributeDataType::Short2,
            );
        }
    }
    vertex_attrs
}

impl TypedLayer for RenderRasterLayer {
    type Type = Raster;

    fn parts(&self) -> (&RenderLayerBase, &LayerValues<Raster>) {
        (&self.base, &self.values)
    }

    fn parts_mut(&mut self) -> (&mut RenderLayerBase, &mut LayerValues<Raster>) {
        (&mut self.base, &mut self.values)
    }
}

impl RenderLayer for RenderRasterLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let properties = self.values.evaluate(parameters);

        self.base.passes =
            if *properties.paint.raster_opacity.uniform() > 0.0 { RenderPass::Translucent } else { RenderPass::None };
        self.values.set_evaluated(&mut self.base, properties);
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
        self.update_tile_entries(params.draws);
    }

    fn release_render_data(&mut self, draws: &mut LayerDraws) {
        crate::render_layer::release_render_data_base(self, draws);
        if let Some(layer_group) = &mut self.image_layer_group {
            layer_group.clear_drawables();
        }
        self.textures.clear();
        self.image_data = None;
    }

    /// The layer's tweaker sets up both its tile group and its image group.
    fn tweak(&mut self, parameters: &PaintParameters<'_>, draws: &mut LayerDraws) {
        if !self.base.is_renderable {
            return;
        }
        let Some(tweaker) = &mut self.base.layer_tweaker else {
            return;
        };
        for group in [&mut draws.group, &mut self.image_layer_group].into_iter().flatten() {
            tweaker.execute(group, &LayerTiles::none(), parameters);
        }
    }

    fn layer_groups<'a>(&'a mut self, draws: &'a mut LayerDraws, groups: &mut Vec<DrawGroup<'a>>) {
        if self.base.is_renderable {
            let layer_groups = [&mut draws.group, &mut self.image_layer_group].into_iter().flatten();
            groups.extend(layer_groups.map(DrawGroup::Drawables));
        }
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) {
        if self.update_projection_variant(state, draws) {
            self.raster_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let render_tiles = self.base.render_tiles.clone().filter(|t| !t.is_empty());
        let image_data = self.image_data.clone();
        if render_tiles.is_none() && image_data.is_none() {
            draws.clear_drawables();
            if let Some(layer_group) = &mut self.image_layer_group {
                layer_group.clear_drawables();
            }
            self.textures.clear();
            return;
        }

        let render_pass = RenderPass::Translucent;

        if self.raster_shader.is_none() {
            self.raster_shader = shaders.shader("RasterShader", self.base.projection_variant);
            if self.raster_shader.is_none() {
                return;
            }
        }
        let raster_shader = self.raster_shader.clone();

        if self.base.layer_tweaker.is_none() {
            self.base.layer_tweaker = Some(Box::new(RasterLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
        }
        let tweaker = self.base.layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);

        if let Some(image_data) = image_data {
            self.update_image(context, raster_shader.as_ref(), &image_data);
            return;
        }
        let Some(render_tiles) = render_tiles else {
            return;
        };

        let mut layer_group = draws.group.take().unwrap_or_else(LayerGroup::tiles);
        {
            {
                let tile_layer_group = &mut layer_group;
                tile_layer_group.composite = self.composite();
                // Remove existing drawables that are no longer in the cover set
                tile_layer_group
                    .remove_drawables_if(|drawable| drawable.tile_id.is_some_and(|id| !draws.has_tile(&id)));
            }

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
            let live_buckets: mln_util::hash::HashSet<SimpleIdentity> =
                render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.style).map(|b| b.get_id())).collect();
            self.textures.retain(|id, _| live_buckets.contains(id));
            // Existing drawables keep their textures, so a changed filter is
            // applied to the textures themselves.
            let sampler = self.sampler_state();
            for texture in self.textures.values() {
                texture.borrow_mut().set_sampler(sampler);
            }

            for tile in render_tiles.iter() {
                let tile_id = *tile.get_overscaled_tile_id();

                let style = Arc::clone(&self.base.style);
                let bucket_ptr = tile.get_bucket(&*style).filter(|b| b.has_data()).cloned();
                let Some(bucket_ptr) = bucket_ptr else {
                    layer_group.remove_drawables(render_pass, &tile_id);
                    continue;
                };
                let Some(bucket) = (*bucket_ptr).downcast_ref::<RasterBucket>() else {
                    layer_group.remove_drawables(render_pass, &tile_id);
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

                if draws.set_bucket(&tile_id, bucket.base.bucket_id) {
                    // Bucket ID changed, we need to rebuild the drawables
                    layer_group.remove_drawables(render_pass, &tile_id);
                    cleared = true;
                } else if geometry_nonempty {
                    // If the bucket data has changed, rebuild the drawables.
                    // Find the earliest time on existing drawables
                    let mut tile_update_time: Option<Instant> = None;
                    {
                        let tile_layer_group = &mut layer_group;
                        tile_layer_group.visit_drawables_for(render_pass, &tile_id, |drawable| {
                            let create_time = drawable.create_time;
                            if tile_update_time.is_none_or(|t| create_time < t) {
                                tile_update_time = Some(create_time);
                            }
                        });
                    }

                    if tile_update_time.is_some_and(|t| t < geometry_modified) {
                        layer_group.remove_drawables(render_pass, &tile_id);
                        cleared = true;
                    }
                }

                if !cleared {
                    // Update existing drawables
                    let mut geometry_changed = false;
                    let update_existing = |drawable: &mut crate::drawable::Drawable| {
                        // Only current drawables are updated, ones produced for
                        // a previous style retain the attribute values for that
                        // style.
                        if drawable.tweaker != tweaker {
                            return false;
                        }
                        // buildVertexData(none, &drawable, bucket): the bucket
                        // may later add, remove, or change masking. In that
                        // case, the tile's shared data and segments are not
                        // updated, and it needs to be re-created.
                        if !geometry_changed && drawable.create_time < geometry_modified {
                            // Masking changed, need to rebuild this tile
                            geometry_changed = true;
                        }
                        true
                    };
                    // If we update existing drawables, don't build new ones. But
                    // if the geometry has changed, we need to drop and re-build
                    // them anyway.
                    if update_tile(&mut layer_group, render_pass, &tile_id, update_existing) && !geometry_changed {
                        continue;
                    } else if geometry_changed {
                        layer_group.remove_drawables(render_pass, &tile_id);
                    }
                }

                // Otherwise, create new ones.
                let mut builder = DrawableBuilder::boxed();
                builder.set_shader(raster_shader.clone());
                builder.set_render_pass(render_pass);
                builder.set_sub_layer_index(0);
                builder.set_depth_type(DepthMaskType::ReadOnly);
                builder.set_color_mode(&ColorMode::alpha_blended());
                builder.set_cull_face_mode(CullFaceMode::disabled());

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
                        let vertices: VertexVectorBasePtr =
                            Rc::<mln_gfx::vertex_vector::VertexVector<RasterLayoutVertex>>::clone(&globe_mesh.vertices);
                        let attrs = raster_vertex_attributes(&vertices);
                        builder.set_vertex_attributes(Some(attrs));
                        builder.set_raw_vertices(Vec::new(), globe_mesh.vertices.elements(), AttributeDataType::Short2);
                        builder.set_segments_shared(
                            DrawMode::Triangles(),
                            Some(Rc::clone(&globe_mesh.indices)),
                            &globe_mesh.segments,
                        );
                    } else if shared {
                        let vertices = shared_vertices(&geometry.vertices);
                        let attrs = raster_vertex_attributes(&vertices);
                        builder.set_vertex_attributes(Some(attrs));
                        builder.set_raw_vertices(Vec::new(), geometry.vertices.len(), AttributeDataType::Short2);
                        builder.set_segments_shared(
                            DrawMode::Triangles(),
                            Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                            &geometry.segments,
                        );
                    } else {
                        let attrs =
                            Rc::clone(static_attrs.get_or_insert_with(|| raster_vertex_attributes(&static_vertices)));
                        builder.set_vertex_attributes(Some(attrs));
                        builder.set_raw_vertices(Vec::new(), 4, AttributeDataType::Short2);
                        builder.set_segments_shared(
                            DrawMode::Triangles(),
                            Some(Rc::clone(&static_indices)),
                            &static_segments,
                        );
                    }
                }

                // finish
                builder.flush();
                let drawables = builder.clear_drawables();
                let tile_layer_group = &mut layer_group;
                for mut drawable in drawables {
                    drawable.tile_id = Some(tile_id);
                    drawable.tweaker = tweaker;
                    tile_layer_group.add_tile_drawable(render_pass, &tile_id, drawable);
                }
            }
        }
        draws.group = Some(layer_group);
    }
}
