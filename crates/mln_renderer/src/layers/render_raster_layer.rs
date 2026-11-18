// The layer keeps one texture per bucket id (`textures`): buckets are Send,
// and textures are render-thread `Rc`s. A tile's draw stays until the
// bucket's masked geometry changes, and is then built again.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
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
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, VertexAttributeArray};
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Raster;
use mln_style_types::{RasterResamplingType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;

use super::{cast_matrix, shared_vertices};
use crate::bucket::{Bucket, NoBucket};
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_composite::Composite;
use crate::layer_drawing::{
    DrawList, DrawObjectsMut, DrawOrder, FrameDraw, FrameDraws, KeptDraw, LayerDrawing, LayerDraws, TileMap,
};
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerParts, LayerPrepareParameters, LayerSource, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase,
    TypedLayer, set_layer_tiles,
};
use crate::render_pass::RenderPass;
use crate::render_source::TileOrder;
use crate::render_static_data::RenderStaticData;
use crate::sources::render_image_source::ImageSourceRenderData;
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;

/// What the layer keeps of a tile: its draw, and when the draw was built.
pub(crate) struct RasterTile {
    draw: KeptDraw,
    built: Instant,
}

/// What a frame's draw of the layer draws.
#[derive(Clone, Copy)]
enum RasterPart {
    /// The draw of its tile.
    Tile,
    /// The image source's draw at the index.
    Image(usize),
}

/// What the layer keeps of its draws: those of the tiles, and of an image
/// source's image, one for each tile it is drawn in.
#[derive(Default)]
struct RasterObjects {
    tiles: TileMap<RasterTile>,
    images: Vec<KeptDraw>,
}

/// The layer's drawing: the GPU objects of its draws, and its draws of the
/// frame.
#[derive(Default)]
pub(crate) struct RasterDrawing {
    objects: RasterObjects,
    frame: Option<LayerDraws<RasterPart>>,
}

impl RasterDrawing {
    /// The GPU objects of `draw`, `part` of the layer's.
    fn objects<'a>(objects: &'a mut RasterObjects, draw: &FrameDraw, part: RasterPart) -> Option<DrawObjectsMut<'a>> {
        let draw = match part {
            RasterPart::Tile => &mut objects.tiles.get_mut(&draw.tile?)?.draw,
            RasterPart::Image(index) => objects.images.get_mut(index)?,
        };
        Some((&mut draw.objects).into())
    }
}

impl LayerDrawing for RasterDrawing {
    type Tile = RasterTile;

    fn tiles(&mut self) -> &mut TileMap<RasterTile> {
        &mut self.objects.tiles
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws)) {
        if let Some(frame) = &self.frame {
            visit(frame.frame());
        }
    }

    fn clear(&mut self) {
        self.objects.tiles.clear();
        self.objects.images.clear();
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, context: &Context) {
        if let Some(frame) = &self.frame {
            frame.upload(&mut self.objects, Self::objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(&mut self.objects, Self::objects, gpu, parameters);
            order.step(parameters);
        }
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
    /// The draws of the tiles, and of an image source's image, one for each
    /// tile it is drawn in.
    drawing: RasterDrawing,
    /// The frame's draws as the update makes them, with their uniform blocks
    /// and projections.
    draw_list: DrawList<(RasterDrawableUBO, ProjectionUBO), RasterPart>,

    raster_shader: Option<ShaderProgramPtr>,

    static_geometry: Option<StaticGeometry>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// `bucket.texture2d`, per bucket.
    textures: HashMap<SimpleIdentity, Texture2DPtr>,
    /// The buckets of the frame's tiles, whose textures stay.
    live_buckets: HashSet<SimpleIdentity>,
}

impl RenderRasterLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Raster>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderRasterLayer {
            base,
            values,
            drawing: RasterDrawing::default(),
            draw_list: DrawList::default(),
            raster_shader: None,
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            live_buckets: HashSet::default(),
        }
    }

    /// The bucket's texture in `textures`, which this creates if needed,
    /// with `sampler`; `None` for a bucket without an image.
    fn bucket_texture(
        textures: &mut HashMap<SimpleIdentity, Texture2DPtr>,
        sampler: SamplerState,
        context: &Context,
        bucket: &RasterBucket,
    ) -> Option<Texture2DPtr> {
        let image = bucket.image.as_ref()?;
        let texture = Rc::clone(textures.entry(bucket.base.bucket_id).or_insert_with(|| {
            let texture = context.create_texture_2d();
            texture.borrow_mut().set_image(Some(Rc::new((**image).clone())));
            texture
        }));

        texture.borrow_mut().set_sampler(sampler);
        Some(texture)
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
    /// The `update` of an image source's layer: one draw for each tile the
    /// image is drawn in, built again every frame.
    fn update_image(
        &mut self,
        context: &Context,
        raster_shader: &ShaderProgramPtr,
        image_data: ImageSourceRenderData<'_>,
        paint_parameters: &PaintParameters<'_>,
    ) {
        self.drawing.objects.images.clear();
        if let Some(frame) = &mut self.drawing.frame {
            frame.clear();
        }

        // The bucket is missing until the image loads.
        let Some(bucket) = image_data.bucket else {
            return;
        };
        let has_vertices = !bucket.geometry().vertices.is_empty();
        if !has_vertices {
            return;
        }

        // TODO: Share textures
        let sampler = self.sampler_state();
        let image_draws = &mut self.drawing.objects.images;
        for (index, tile_id) in image_data.tile_ids.iter().enumerate() {
            let geometry = {
                // The bucket's own geometry, which an image source's bucket
                // always has.
                let geometry = bucket.geometry();
                let vertices = shared_vertices(&geometry.vertices);
                raster_geometry(
                    &vertices,
                    geometry.vertices.len(),
                    Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone()))),
                    &geometry.segments,
                )
            };
            let texture = Self::bucket_texture(&mut self.textures, sampler, context, bucket);
            let draw = KeptDraw::new(new_objects(raster_shader, geometry, texture));
            let uniforms = Self::uniforms(tile_id, paint_parameters);
            self.draw_list.push_part(FrameDraw::new(None, draw.id), RasterPart::Image(index), uniforms);
            image_draws.push(draw);
        }

        self.finish_frame_draws(context);
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

    /// Makes the draws of the draw list the layer's draws of the frame, and
    /// writes the layer's buffers of them.
    fn finish_frame_draws(&mut self, context: &Context) {
        let composite = self.composite();
        let frame = self.drawing.frame.get_or_insert_with(LayerDraws::new);
        frame.set_composite(composite);
        let evaluated = &self.values.evaluated.paint;
        self.draw_list.finish(frame, context, |writer| {
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
            writer.uniform(ID_RASTER_EVALUATED_PROPS_UBO, &props);
            writer.storage(ID_RASTER_DRAWABLE_UBO, |(draw_ubo, _)| *draw_ubo);
            writer.storage(ID_PROJECTION_UBO, |(_, projection)| *projection);
        });
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

/// The GPU objects of a raster draw of `geometry` with `shader`, which binds
/// `texture` as both of its images.
fn new_objects(shader: &ShaderProgramPtr, geometry: DrawGeometry, texture: Option<Texture2DPtr>) -> DrawObjects {
    let mut textures = Textures::default();
    if let Some(texture) = texture {
        textures[ID_RASTER_IMAGE0_TEXTURE] = Some(Rc::clone(&texture));
        textures[ID_RASTER_IMAGE1_TEXTURE] = Some(texture);
    }
    DrawObjects::new(Rc::clone(shader), ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, textures)
}

/// The geometry of a raster draw of `vertex_count` vertices in `vertices`,
/// and `segments` of `indexes`.
fn raster_geometry(
    vertices: &VertexVectorBasePtr,
    vertex_count: usize,
    indexes: IndexVectorBasePtr,
    segments: &[SegmentBase],
) -> DrawGeometry {
    DrawGeometry {
        vertices: raster_vertex_attributes(vertices),
        vertex_count,
        mode: DrawMode::Triangles(),
        indexes,
        segments: segments.iter().map(SegmentBase::copy).collect(),
        instances: None,
    }
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
    type Drawing = RasterDrawing;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }
}

impl RenderLayer for RenderRasterLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let properties = self.values.evaluate(parameters);

        self.base.passes =
            if *properties.paint.raster_opacity.uniform() > 0.0 { RenderPass::Translucent } else { RenderPass::None };
        self.values.set_evaluated(&mut self.base, properties);
    }

    /// Picks the source's tiles; an image source has none, and its image
    /// comes with the update. The passes are the layer's own, whatever its
    /// tiles' buckets were laid out for.
    fn prepare(&mut self, params: &LayerPrepareParameters<'_>) -> Option<TileOrder> {
        let order = params.source.map(|_| TileOrder::ById);
        set_layer_tiles(self, params, order)
    }

    fn release_render_data(&mut self) {
        crate::render_layer::release_render_data_base(self);
        self.textures.clear();
    }

    /// Makes the frame's draws of the layer: those of an image source's
    /// image, or one of each of its tiles, which it builds for a tile it
    /// keeps none of or whose bucket's geometry changed since, and writes
    /// the layer's buffers they read.
    fn update(
        &mut self,
        paint_parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        source: Option<LayerSource<'_>>,
    ) {
        let (shaders, context, state) =
            (&paint_parameters.static_data.shaders, paint_parameters.context, paint_parameters.state);
        if self.update_projection_variant(state) {
            self.raster_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let render_tiles = source.and_then(LayerSource::tiles).filter(|tiles| !tiles.is_empty());
        let image_data = source.and_then(LayerSource::image);
        if render_tiles.is_none() && image_data.is_none() {
            self.remove_all_draws();
            self.textures.clear();
            return;
        }

        if self.raster_shader.is_none() {
            self.raster_shader = shaders.shader("RasterShader", self.base.projection_variant);
        }
        let Some(raster_shader) = self.raster_shader.clone() else {
            return;
        };

        if let Some(image_data) = image_data {
            self.update_image(context, &raster_shader, image_data, paint_parameters);
            return;
        }
        let Some(render_tiles) = render_tiles else {
            return;
        };
        self.drawing.frame.get_or_insert_with(LayerDraws::new);

        let sampler = self.sampler_state();
        let static_geometry = self.static_geometry.get_or_insert_with(|| StaticGeometry {
            vertices: shared_vertices(&RenderStaticData::raster_vertices()),
            indexes: Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices()))),
            segments: RenderStaticData::raster_segments(),
        });

        // Drop the textures of buckets that are no longer rendered.
        let live_buckets = &mut self.live_buckets;
        live_buckets.clear();
        live_buckets.extend(render_tiles.iter().filter_map(|tile| tile.bucket(&*self.base.style).map(Bucket::get_id)));
        self.textures.retain(|id, _| live_buckets.contains(id));
        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        for texture in self.textures.values() {
            texture.borrow_mut().set_sampler(sampler);
        }

        for tile in render_tiles.iter() {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.objects.tiles.entry(&tile_id) else {
                continue;
            };

            let bucket = tile.bucket(&*self.base.style).filter(|b| b.has_data());
            let Some(bucket) = bucket.and_then(|bucket| bucket.downcast_ref::<RasterBucket>()) else {
                *entry.value = None;
                continue;
            };

            // A draw of another bucket, or of the bucket's geometry before
            // its mask changed, is built again.
            entry.set_bucket(bucket.base.bucket_id);
            let geometry_modified = bucket.geometry().last_modified;
            if entry.value.as_ref().is_some_and(|cached| cached.built < geometry_modified) {
                *entry.value = None;
            }

            if entry.value.is_none() {
                let texture = Self::bucket_texture(&mut self.textures, sampler, context, bucket);

                // The bucket only has geometry for a masked tile; any other
                // tile is drawn with the static quad over the tile's extent.
                // On the globe, the draw is the pole-capped grid of what the
                // mask keeps.
                let geometry = bucket.geometry();
                let globe = self.base.projection_variant == ProjectionVariant::Globe;
                let shared = !globe
                    && !geometry.vertices.is_empty()
                    && !geometry.indices.is_empty()
                    && !geometry.segments.is_empty();
                let draw_geometry = if globe {
                    let globe_mesh =
                        self.globe_meshes.get(&tile_id.canonical, &geometry.mask, RasterBucket::layout_vertex);
                    let vertices: VertexVectorBasePtr =
                        Rc::<mln_gfx::vertex_vector::VertexVector<RasterLayoutVertex>>::clone(&globe_mesh.vertices);
                    raster_geometry(
                        &vertices,
                        globe_mesh.vertices.elements(),
                        Rc::clone(&globe_mesh.indices),
                        &globe_mesh.segments,
                    )
                } else if shared {
                    let vertices = shared_vertices(&geometry.vertices);
                    raster_geometry(
                        &vertices,
                        geometry.vertices.len(),
                        Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone()))),
                        &geometry.segments,
                    )
                } else {
                    raster_geometry(
                        &static_geometry.vertices,
                        4,
                        Rc::clone(&static_geometry.indexes),
                        &static_geometry.segments,
                    )
                };
                let objects = new_objects(&raster_shader, draw_geometry, texture);
                let built = RasterTile { draw: KeptDraw::new(objects), built: Instant::now() };
                *entry.value = Some(built);
            }

            if let Some(cached) = entry.value.as_ref() {
                let draw = cached.draw.frame_draw(Some(tile_id));
                let uniforms = Self::uniforms(&tile_id.to_unwrapped(), paint_parameters);
                self.draw_list.push_part(draw, RasterPart::Tile, uniforms);
            }
        }

        self.finish_frame_draws(context);
    }
}
