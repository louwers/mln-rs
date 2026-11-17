// Differences in mechanics, not in output:
// - C++ keeps the GPU texture in the (shared) RasterBucket; here the layer
//   keeps one texture per bucket id (`textures`), because buckets are Send +
//   Sync and textures are render-thread `Rc`s.
// - C++ re-applies the static vertex attributes to existing tile drawables
//   every frame (`buildVertexData(none, &drawable, bucket)`); the port only
//   checks, as C++ does there, whether the bucket's masked geometry changed
//   since the tile's draw was built, and builds it again if so. The static
//   geometry never changes, so the draws are the same.
//

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_RASTER_DRAWABLE_UBO, ID_RASTER_EVALUATED_PROPS_UBO, ID_RASTER_IMAGE0_TEXTURE,
    ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, ShaderRegistry, VertexAttributeArray,
};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Raster;
use mln_style_types::{RasterResamplingType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;

use super::{cast_matrix, shared_vertices};
use crate::bucket::NoBucket;
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::draw::DrawObjects;
use crate::draw_cache::{DrawCache, FrameDraw, FrameDraws, KeptDraw, LayerDraws};
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_composite::Composite;
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerPrepareParameters, LayerTiles, LayerValues, RenderLayer, RenderLayerBase, TypedLayer, set_layer_tiles,
};
use crate::render_pass::RenderPass;
use crate::render_source::RenderSource;
use crate::render_static_data::RenderStaticData;
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;

/// What the layer keeps of a tile: its draw, and when the draw was built.
struct RasterTile {
    draw: KeptDraw,
    built: Instant,
}

impl DrawCache for RasterTile {
    fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects> {
        self.draw.objects_mut(id)
    }
}

/// The draws of an image source's image, one for each tile it is drawn in,
/// with the tile.
struct ImageDraws(Vec<(UnwrappedTileID, KeptDraw)>);

impl DrawCache for ImageDraws {
    fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects> {
        self.0.iter_mut().find(|(_, draw)| draw.id == id).map(|(_, draw)| &mut draw.objects)
    }
}

/// The tile extent's geometry, which a tile draws that its bucket doesn't
/// mask.
struct StaticGeometry {
    vertices: VertexVectorBasePtr,
    indexes: IndexVectorBasePtr,
    segments: Vec<SegmentBase>,
}

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

    static_geometry: Option<StaticGeometry>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// `bucket.texture2d`, per bucket.
    textures: HashMap<SimpleIdentity, Texture2DPtr>,

    image_data: Option<Rc<ImageSourceRenderData>>,
}

impl RenderRasterLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Raster>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderRasterLayer {
            base,
            values,
            raster_shader: None,
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            image_data: None,
        }
    }

    /// Creates the bucket's texture if needed and attaches it to both image
    /// slots of `objects`.
    fn set_textures(&mut self, context: &Context, objects: &mut DrawObjects, bucket: &RasterBucket) {
        let Some(image) = &bucket.image else {
            return;
        };
        let texture = Rc::clone(self.textures.entry(bucket.base.bucket_id).or_insert_with(|| {
            let texture = context.create_texture_2d();
            texture.borrow_mut().set_image(Some(Rc::new((**image).clone())));
            texture
        }));

        texture.borrow_mut().set_sampler(self.sampler_state());

        objects.set_texture(Some(Rc::clone(&texture)), ID_RASTER_IMAGE0_TEXTURE);
        objects.set_texture(Some(texture), ID_RASTER_IMAGE1_TEXTURE);
    }

    /// How the layer's draws composite onto the frame: as one surface, for a
    /// `raster-layer-blend` other than `normal`.
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
    /// The `imageData` branch of `update`: one draw for each tile the image
    /// is drawn in, built again every frame (C++: "TODO: Can we avoid
    /// rebuilding drawables each time in this case as well?").
    fn update_image(
        &mut self,
        context: &Context,
        raster_shader: Option<&ShaderProgramPtr>,
        image_data: &ImageSourceRenderData,
        paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) {
        draws.drop_layer_cache();
        if let Some(frame) = &mut draws.frame {
            frame.draws.clear();
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

        // TODO: Share textures
        let mut image_draws = Vec::new();
        for tile_id in &image_data.tile_ids {
            let mut objects = new_objects(raster_shader);
            {
                // buildVertexData(builder, nullptr, bucket): the bucket's own
                // geometry (shared: vertices, indices and segments are set).
                let geometry = bucket.geometry();
                let vertices = shared_vertices(&geometry.vertices);
                objects.update_vertex_attributes(
                    raster_vertex_attributes(&vertices),
                    geometry.vertices.len(),
                    DrawMode::Triangles(),
                    Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                    &geometry.segments,
                );
            }
            self.set_textures(context, &mut objects, &bucket);
            image_draws.push((*tile_id, KeptDraw::new(objects)));
        }

        let frame_draws: Vec<_> = image_draws
            .iter()
            .map(|(tile_id, draw)| (FrameDraw::new(None, draw.id), Self::uniforms(tile_id, paint_parameters)))
            .collect();
        draws.set_layer_cache(ImageDraws(image_draws));
        let frame = draws.frame.get_or_insert_with(FrameDraws::new);
        frame.composite = self.composite();
        self.set_frame_draws(frame, frame_draws, context);
    }

    /// The entries in the layer's storage buffers of a draw of `tile_id`.
    fn uniforms(tile_id: &UnwrappedTileID, parameters: &PaintParameters<'_>) -> (RasterDrawableUBO, ProjectionUBO) {
        let projection = get_projection_data(
            tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            !parameters.state.is_changing(),
        );
        (RasterDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) }, to_projection_ubo(&projection))
    }

    /// Makes `draws`, sorted into drawing order, the draws of `frame`, and
    /// writes the layer's buffers of them.
    fn set_frame_draws(
        &self,
        frame: &mut FrameDraws,
        mut draws: Vec<(FrameDraw, (RasterDrawableUBO, ProjectionUBO))>,
        context: &Context,
    ) {
        draws.sort_by_key(|(draw, _)| draw.order());
        if !draws.is_empty() {
            let evaluated = &self.values.evaluated.paint;
            let props = RasterEvaluatedPropsUBO {
                spin_weights: spin_weights(*evaluated.raster_hue_rotate.uniform()),
                tl_parent: [0.0, 0.0],
                scale_parent: 1.0,
                buffer_scale: 1.0,
                fade_t: 1.0,
                opacity: *evaluated.raster_opacity.uniform(),
                brightness_low: *evaluated.raster_brightness_min.uniform(),
                brightness_high: *evaluated.raster_brightness_max.uniform(),
                saturation_factor: saturation_factor(*evaluated.raster_saturation.uniform()),
                contrast_factor: contrast_factor(*evaluated.raster_contrast.uniform()),
                pad1: 0.0,
                pad2: 0.0,
            };
            let buffers = &mut frame.uniform_buffers;
            buffers.create_or_update_pod(ID_RASTER_EVALUATED_PROPS_UBO, &props, context);
            let drawables: Vec<RasterDrawableUBO> = draws.iter().map(|(_, (drawable, _))| *drawable).collect();
            buffers.write_storage(ID_RASTER_DRAWABLE_UBO, bytemuck::cast_slice(&drawables), context);
            let projections: Vec<ProjectionUBO> = draws.iter().map(|(_, (_, projection))| *projection).collect();
            buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projections), context);
        }
        frame.draws = draws.into_iter().map(|(draw, _)| draw).collect();
    }
}

fn spin_weights(spin: f32) -> [f32; 4] {
    let spin = mln_math::deg2radf(spin);
    let s = spin.sin();
    let c = spin.cos();
    let sqrt3 = 3.0f32.sqrt(); // std::numbers::sqrt3_v<float>
    [(2.0 * c + 1.0) / 3.0, (-sqrt3 * s - c + 1.0) / 3.0, (sqrt3 * s - c + 1.0) / 3.0, 0.0]
}

fn saturation_factor(saturation: f32) -> f32 {
    if saturation > 0.0 { 1.0 - 1.0 / (1.001 - saturation) } else { -saturation }
}

fn contrast_factor(contrast: f32) -> f32 {
    if contrast > 0.0 { 1.0 / (1.0 - contrast) } else { 1.0 + contrast }
}

/// The GPU objects of a raster draw with `shader`, without geometry or
/// textures.
fn new_objects(shader: Option<&ShaderProgramPtr>) -> DrawObjects {
    let mut objects = DrawObjects::new(String::new());
    objects.set_color_mode(&ColorMode::alpha_blended());
    objects.set_cull_face_mode(CullFaceMode::disabled());
    objects.set_shader(shader.cloned());
    objects
}

/// The vertex attributes of a raster draw over `vertices`.
fn raster_vertex_attributes(vertices: &VertexVectorBasePtr) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
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
    attrs
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

    /// Picks the source's tiles, or an image source's image, which may not
    /// be available until the source loads it. The passes are the layer's
    /// own, whatever its tiles' buckets were laid out for.
    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        let order = params.source.and_then(RenderSource::tile_order);
        self.image_data = params.source.and_then(RenderSource::get_image_render_data);
        set_layer_tiles(params, order)
    }

    fn release_render_data(&mut self, draws: &mut LayerDraws) {
        crate::render_layer::release_render_data_base(self, draws);
        self.textures.clear();
        self.image_data = None;
    }

    /// Makes the frame's draws of the layer: those of an image source's
    /// image, or one of each of its tiles, which it builds for a tile it
    /// keeps none of or whose bucket's geometry changed since, and writes
    /// the layer's buffers they read.
    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        paint_parameters: &PaintParameters<'_>,
        tiles: Option<LayerTiles<'_>>,
        draws: &mut LayerDraws,
    ) {
        if self.update_projection_variant(state, draws) {
            self.raster_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let render_tiles = tiles.filter(|tiles| !tiles.is_empty());
        let image_data = self.image_data.clone();
        if render_tiles.is_none() && image_data.is_none() {
            draws.clear_draws();
            self.textures.clear();
            return;
        }

        if self.raster_shader.is_none() {
            self.raster_shader = shaders.shader("RasterShader", self.base.projection_variant);
            if self.raster_shader.is_none() {
                return;
            }
        }
        let raster_shader = self.raster_shader.clone();

        if let Some(image_data) = image_data {
            self.update_image(context, raster_shader.as_ref(), &image_data, paint_parameters, draws);
            return;
        }
        let Some(render_tiles) = render_tiles else {
            return;
        };
        draws.frame.get_or_insert_with(FrameDraws::new);

        let static_geometry = self.static_geometry.get_or_insert_with(|| StaticGeometry {
            vertices: shared_vertices(&RenderStaticData::raster_vertices()),
            indexes: Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices()))),
            segments: RenderStaticData::raster_segments(),
        });
        let static_vertices = Rc::clone(&static_geometry.vertices);
        let static_indexes = Rc::clone(&static_geometry.indexes);
        let static_segments: Vec<SegmentBase> = static_geometry.segments.iter().map(SegmentBase::copy).collect();

        // Drop the textures of buckets that are no longer rendered.
        let live_buckets: mln_util::hash::HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.style).map(|b| b.get_id())).collect();
        self.textures.retain(|id, _| live_buckets.contains(id));
        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        let sampler = self.sampler_state();
        for texture in self.textures.values() {
            texture.borrow_mut().set_sampler(sampler);
        }

        let mut frame_draws = Vec::new();
        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let bucket_ptr = tile.get_bucket(&*self.base.style).filter(|b| b.has_data()).cloned();
            let Some(bucket) = bucket_ptr.as_deref().and_then(|bucket| bucket.downcast_ref::<RasterBucket>()) else {
                draws.drop_cache(&tile_id);
                continue;
            };

            // A draw of another bucket, or of the bucket's geometry before
            // its mask changed, is built again.
            let geometry_modified = bucket.geometry().last_modified;
            if draws.set_bucket(&tile_id, bucket.base.bucket_id)
                || draws.cache_mut::<RasterTile>(&tile_id).is_some_and(|cached| cached.built < geometry_modified)
            {
                draws.drop_cache(&tile_id);
            }

            if draws.cache_mut::<RasterTile>(&tile_id).is_none() {
                let mut objects = new_objects(raster_shader.as_ref());
                self.set_textures(context, &mut objects, bucket);

                // buildVertexData(builder, nullptr, bucket, tileID): the
                // bucket only fills in geometry for masked tiles, otherwise
                // the standard tile extent geometry is used; on the globe,
                // the pole-capped grid of what the mask keeps.
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
                    objects.update_vertex_attributes(
                        raster_vertex_attributes(&vertices),
                        globe_mesh.vertices.elements(),
                        DrawMode::Triangles(),
                        Some(Rc::clone(&globe_mesh.indices)),
                        &globe_mesh.segments,
                    );
                } else if shared {
                    let vertices = shared_vertices(&geometry.vertices);
                    objects.update_vertex_attributes(
                        raster_vertex_attributes(&vertices),
                        geometry.vertices.len(),
                        DrawMode::Triangles(),
                        Some(Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone())))),
                        &geometry.segments,
                    );
                } else {
                    objects.update_vertex_attributes(
                        raster_vertex_attributes(&static_vertices),
                        4,
                        DrawMode::Triangles(),
                        Some(Rc::clone(&static_indexes)),
                        &static_segments,
                    );
                }
                drop(geometry);
                draws.set_cache(&tile_id, RasterTile { draw: KeptDraw::new(objects), built: Instant::now() });
            }

            if let Some(cached) = draws.cache_mut::<RasterTile>(&tile_id) {
                let uniforms = Self::uniforms(&tile_id.to_unwrapped(), paint_parameters);
                frame_draws.push((FrameDraw::new(Some(tile_id), cached.draw.id), uniforms));
            }
        }

        let composite = self.composite();
        let Some(frame) = &mut draws.frame else {
            return;
        };
        frame.composite = composite;
        self.set_frame_draws(frame, frame_draws, context);
    }
}
