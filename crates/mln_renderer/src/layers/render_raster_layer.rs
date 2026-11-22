// The layer keeps one texture per bucket id (`textures`), in the context's
// store: buckets are Send and cross from the workers without one. A tile's
// draw stays until the bucket's masked geometry changes, and is then built
// again.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_RASTER_DRAWABLE_UBO, ID_RASTER_EVALUATED_PROPS_UBO, ID_RASTER_IMAGE0_TEXTURE,
    ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Raster;
use mln_style_types::{RasterResamplingType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;

use super::cast_matrix;
use crate::bucket::NoBucket;
use crate::bucket_geometry::copied_vertices;
use crate::buckets::raster_bucket::{RasterBucket, RasterLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::context::Context;
use crate::gfx::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::gfx::store::SharedTexture;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_composite::Composite;
use crate::layer_drawing::{
    DrawGroup, DrawList, DrawObjectsMut, DrawOrder, FrameDraw, KeptDraw, LayerDrawing, LayerDraws, TileMap,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::sources::render_image_source::{ImageQuad, ImageSourceRenderData};
use crate::tile_matrix::{projection_data, to_projection_ubo};

/// What the layer keeps of a tile: its draw, and the generation of the
/// bucket's geometry it was built of.
pub(crate) struct RasterTile {
    draw: KeptDraw,
    geometry_generation: u64,
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
/// source's image, one for each tile it is drawn in, with the image's
/// bucket and the generation of its geometry they were built of.
#[derive(Default)]
struct RasterObjects {
    tiles: TileMap<RasterTile>,
    images: Vec<KeptDraw>,
    image: Option<(SimpleIdentity, u64)>,
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

    fn tiles(&mut self) -> Option<&mut TileMap<RasterTile>> {
        Some(&mut self.objects.tiles)
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(frame.frame()));
        }
    }

    fn clear(&mut self) {
        self.objects.tiles.clear();
        self.objects.images.clear();
        self.objects.image = None;
        self.clear_frame();
    }

    fn begin_frame(&mut self) {
        self.frame.get_or_insert_default().clear();
    }

    fn clear_frame(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Some(frame) = &self.frame {
            frame.upload(gpu, &mut self.objects, Self::objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(&mut self.objects, Self::objects, gpu, parameters);
            order.step(parameters);
        }
    }

    fn frame_buffers(&mut self) -> Option<&mut UniformBufferArray> {
        Some(self.frame.as_mut()?.uniform_buffers_mut())
    }
}

/// The tile extent's geometry, which a tile draws that its bucket doesn't
/// mask.
struct StaticGeometry {
    vertices: SharedVertices,
    indexes: SharedIndexes,
    segments: Vec<SegmentBase>,
}

impl StaticGeometry {
    /// The tile extent's quad, in `context`'s stores.
    fn new(context: &mut Context) -> StaticGeometry {
        StaticGeometry {
            vertices: copied_vertices(context, &RenderStaticData::raster_vertices()),
            indexes: context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())),
            segments: RenderStaticData::raster_segments(),
        }
    }
}

impl LayerType for Raster {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type TileBucket<'a> = &'a RasterBucket;

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

    static_geometry: Option<StaticGeometry>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The texture of each bucket's image, by bucket id.
    textures: HashMap<SimpleIdentity, SharedTexture>,
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
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            live_buckets: HashSet::default(),
        }
    }

    /// The bucket's texture in `textures`, which this creates if needed,
    /// with `sampler`; `None` for a bucket without an image.
    fn bucket_texture(
        textures: &mut HashMap<SimpleIdentity, SharedTexture>,
        sampler: SamplerState,
        context: &mut Context,
        bucket: &RasterBucket,
    ) -> Option<SharedTexture> {
        let image = bucket.image.as_ref()?;
        let texture = textures
            .entry(bucket.base.bucket_id)
            .or_insert_with(|| context.add_texture_with(|texture| texture.set_image(Some(Rc::new((**image).clone())))))
            .clone();
        if let Some(mut texture) = context.texture_mut(&texture) {
            texture.set_sampler(sampler);
        }
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
        // Either property asking for Nearest draws Nearest.
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
    /// The draws of an image source's layer with `raster_shader`: one for
    /// each tile the image is drawn in, which share the image's geometry
    /// and texture, built again when the image or its geometry changes.
    fn update_image(
        &mut self,
        gpu: &mut dyn Gpu,
        image_data: ImageSourceRenderData<'_>,
        paint_parameters: &mut PaintParameters<'_>,
    ) {
        let sampler = self.sampler_state();
        let RenderRasterLayer { base, drawing, draw_list, textures, .. } = self;
        let raster_shader =
            paint_parameters.context.shader_key(&programs::RASTER, Uniforms::default(), base.projection_variant);
        let objects = &mut drawing.objects;
        // The bucket is missing until the image loads. An image source's
        // bucket always has geometry of its own once it has an image.
        let Some((bucket, quads)) = image_data.bucket.and_then(|bucket| Some((bucket, bucket.geometry().quads()?)))
        else {
            objects.images.clear();
            objects.image = None;
            return;
        };
        let built_of = (bucket.base.bucket_id, bucket.geometry().generation());
        if objects.image != Some(built_of) {
            objects.images.clear();
            objects.image = Some(built_of);
            // Only the image's bucket draws: the textures of the images
            // before it go.
            textures.retain(|id, _| *id == bucket.base.bucket_id);
        }
        let texture = Self::bucket_texture(textures, sampler, paint_parameters.context, bucket);
        let tile_ids = image_data.tile_ids;
        objects.images.truncate(tile_ids.len());
        while objects.images.len() < tile_ids.len() {
            let draw_geometry =
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments);
            let color_mode = paint_parameters.context.color_mode(ColorMode::alpha_blended());
            objects.images.push(KeptDraw::new(new_objects(
                &raster_shader,
                color_mode,
                draw_geometry,
                texture.as_ref(),
            )));
        }
        for (index, (tile_id, draw)) in tile_ids.iter().zip(&objects.images).enumerate() {
            let uniforms = Self::uniforms(tile_id, paint_parameters);
            draw_list.push_part(FrameDraw::new(None, draw.id), RasterPart::Image(index), uniforms);
        }

        self.finish_frame_draws(gpu, paint_parameters.context, &image_data.quad);
    }

    /// The entries in the layer's storage buffers of a draw of `tile_id`.
    fn uniforms(tile_id: &UnwrappedTileID, parameters: &PaintParameters<'_>) -> (RasterDrawableUBO, ProjectionUBO) {
        let projection = projection_data(
            tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            !parameters.camera_moving,
        );
        (RasterDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) }, to_projection_ubo(&projection))
    }

    /// Makes the draws of the draw list the layer's draws of the frame, and
    /// writes the layer's buffers of them, which put their vertices on
    /// `quad`.
    fn finish_frame_draws(&mut self, gpu: &mut dyn Gpu, context: &mut Context, quad: &ImageQuad) {
        let composite = self.composite();
        let Some(frame) = self.drawing.frame.as_mut() else {
            return;
        };
        frame.set_composite(composite);
        let evaluated = &self.values.evaluated.paint;
        let replaced = self.draw_list.finish(frame, gpu, context, |writer| {
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
                coords_top: quad.coords_top,
                coords_bottom: quad.coords_bottom,
                image_warp: quad.warp,
            };
            writer.uniform(ID_RASTER_EVALUATED_PROPS_UBO, &props);
            writer.storage(ID_RASTER_DRAWABLE_UBO, |(draw_ubo, _)| *draw_ubo);
            writer.storage(ID_PROJECTION_UBO, |(_, projection)| *projection);
        });
        if replaced {
            let objects = &mut self.drawing.objects;
            for draw in objects.tiles.values_mut().map(|tile| &mut tile.draw).chain(&mut objects.images) {
                draw.objects.drop_bindings();
            }
        }
    }
}

fn spin_weights(spin: f32) -> [f32; 4] {
    let spin = mln_math::deg2radf(spin);
    let s = spin.sin();
    let c = spin.cos();
    let sqrt3 = 3.0f32.sqrt();
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
fn new_objects(
    shader: &ShaderKey,
    color_mode: ColorMode,
    geometry: DrawGeometry,
    texture: Option<&SharedTexture>,
) -> DrawObjects {
    let mut textures = Textures::default();
    textures[ID_RASTER_IMAGE0_TEXTURE] = texture.cloned();
    textures[ID_RASTER_IMAGE1_TEXTURE] = texture.cloned();
    DrawObjects::new(shader.clone(), color_mode, CullFaceMode::disabled(), geometry, textures)
}

/// The geometry of a raster draw of `vertex_count` vertices in `vertices`,
/// and `segments` of `indexes`.
fn raster_geometry(
    vertices: &SharedVertices,
    vertex_count: usize,
    indexes: DrawIndexes,
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
fn raster_vertex_attributes(vertices: &SharedVertices) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let stride = size_of::<RasterLayoutVertex>() as u32;
    attrs.set_shared(
        ID_RASTER_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a1) as u32,
            stride,
        ),
    );
    attrs.set_shared(
        ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a2) as u32,
            stride,
        ),
    );
    attrs
}

impl TypedLayer for RenderRasterLayer {
    type Type = Raster;
    type Drawing = RasterDrawing;
    const DRAPES: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Drops the globe's meshes when the frame isn't on the globe.
    fn projection_changed(&mut self) {
        if self.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Raster>) -> RenderPass {
        if *evaluated.paint.raster_opacity.uniform() > 0.0 { RenderPass::Translucent } else { RenderPass::None }
    }

    /// Makes the frame's draws of the layer, one of each of its tiles, which
    /// it builds for a tile it keeps none of or whose bucket's geometry
    /// changed since, and writes the layer's buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Raster>],
    ) {
        let sampler = self.sampler_state();
        let RenderRasterLayer { base, drawing, draw_list, static_geometry, globe_meshes, textures, .. } = self;
        let raster_shader =
            paint_parameters.context.shader_key(&programs::RASTER, Uniforms::default(), base.projection_variant);
        let static_geometry = static_geometry.get_or_insert_with(|| StaticGeometry::new(paint_parameters.context));

        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        for texture in textures.values() {
            if let Some(mut texture) = paint_parameters.context.texture_mut(texture) {
                texture.set_sampler(sampler);
            }
        }

        for tile in render_tiles {
            let tile_id = tile.draw_id(paint_parameters);
            let Some(mut entry) = drawing.objects.tiles.entry(&tile_id) else {
                continue;
            };

            let Some(bucket) = tile.bucket().filter(|bucket| bucket.has_data()) else {
                *entry.value = None;
                continue;
            };

            // A draw of another bucket, or of the bucket's geometry before
            // its mask changed, is built again.
            entry.set_bucket(bucket.base.bucket_id);
            let geometry_generation = bucket.geometry().generation();
            if entry.value.as_ref().is_some_and(|cached| cached.geometry_generation != geometry_generation) {
                *entry.value = None;
            }

            if entry.value.is_none() {
                let texture = Self::bucket_texture(textures, sampler, paint_parameters.context, bucket);

                // The bucket only has geometry for a masked tile; any other
                // tile is drawn with the static quad over the tile's extent.
                // On the globe, the draw is the pole-capped grid of what the
                // mask keeps.
                let geometry = bucket.geometry();
                let context = &mut *paint_parameters.context;
                let draw_geometry = if base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        globe_meshes.get(context, &tile_id.canonical, &geometry.mask, RasterLayoutVertex::new);
                    raster_geometry(
                        &globe_mesh.vertices,
                        globe_mesh.vertices.count(),
                        globe_mesh.indexes(),
                        &globe_mesh.segments,
                    )
                } else if let Some(quads) = geometry.quads() {
                    raster_geometry(
                        &quads.vertices,
                        quads.vertices.count(),
                        quads.indexes.clone().into(),
                        &quads.segments,
                    )
                } else {
                    raster_geometry(
                        &static_geometry.vertices,
                        4,
                        static_geometry.indexes.clone().into(),
                        &static_geometry.segments,
                    )
                };
                let color_mode = context.color_mode(ColorMode::alpha_blended());
                let objects = new_objects(&raster_shader, color_mode, draw_geometry, texture.as_ref());
                let built = RasterTile { draw: KeptDraw::new(objects), geometry_generation };
                *entry.value = Some(built);
            }

            if let Some(cached) = entry.value.as_ref() {
                let draw = cached.draw.frame_draw(Some(tile_id));
                let uniforms = Self::uniforms(&tile_id.to_unwrapped(), paint_parameters);
                draw_list.push_part(draw, RasterPart::Tile, uniforms);
            }
        }

        self.finish_frame_draws(gpu, paint_parameters.context, &ImageQuad::TILE);
    }

    fn update_image_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        image: ImageSourceRenderData<'_>,
    ) {
        self.update_image(gpu, image, paint_parameters);
    }

    /// Drops the textures of the buckets of tiles the frame doesn't draw.
    /// The frame's draws and its drapes' share the others. An image source's
    /// draws keep their image's texture themselves.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, Raster>]) {
        if params.source.is_some_and(|source| source.image_render_data().is_some()) {
            return;
        }
        let live_buckets = &mut self.live_buckets;
        live_buckets.clear();
        live_buckets.extend(tiles.iter().filter_map(|tile| Some(tile.bucket()?.base.bucket_id)));
        self.textures.retain(|id, _| live_buckets.contains(id));
    }

    fn release(&mut self) {
        self.textures.clear();
    }

    fn nothing_to_draw(&mut self) {
        self.textures.clear();
    }
}

impl RenderLayer for RenderRasterLayer {}
