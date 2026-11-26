// The layer keeps one texture and the quads of one mask per tile contents'
// id (`textures`, `mask_quads`), in the context's store, which its draws of a
// tile in the frame and in drapes share. A tile's draw stays until the
// tile's mask changes, and is then built again.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::LayerStyle;
use mln_style::layers::Raster;
use mln_style::types::{LayerBlendType, RasterResamplingType, ResamplingType, TranslateAnchorType};
use mln_util::identity::SimpleIdentity;

use super::cast_matrix;
use crate::buckets::raster_bucket::{MaskQuads, RasterLayoutVertex};
use crate::draw::context::Context;
use crate::draw::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::draw::objects::{DrawGeometry, DrawObjects, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::store::SharedTexture;
use crate::draw::uniform_buffer::UniformBufferArray;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::draw::vertices::SharedVertices;
use crate::layer::bucket::NoBucket;
use crate::layer::bucket_geometry::copied_vertices;
use crate::layer::layer_drawing::{
    DrawGroup, DrawList, DrawObjectsMut, DrawOrder, FrameDraw, KeptDraw, LayerDrawing, LayerDraws, TileMap,
};
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::layout::NoLayout;
use crate::layer::render_layer::{
    Atlases, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::layers::raster::programs;
use crate::layers::raster::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use crate::render::globe::globe_tile_mesh::GlobeTileMeshCache;
use crate::render::layer_composite::Composite;
use crate::render::paint_parameters::PaintParameters;
use crate::render::pass::RenderPass;
use crate::render::static_data::RenderStaticData;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{projection_data, to_projection_ubo};
use crate::source::render_image_source::{ImageQuad, ImageSourceRenderData};
use crate::tile::UnwrappedTileID;
use crate::tile_store::raster::RasterContents;

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::RASTER.buffer_id("drawableVector");
const PROPS_ID: usize = programs::RASTER.buffer_id("props");

/// The slots of the programs' textures that the draws bind.
const IMAGE0_SLOT: usize = programs::RASTER.texture_slot("image0");
const IMAGE1_SLOT: usize = programs::RASTER.texture_slot("image1");

/// What the layer keeps of a tile: its draw, and the generation of the
/// tile's mask it was built of.
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

/// The tile extent's geometry, which a tile draws that its mask keeps
/// whole.
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
    type TileBucket<'a> = &'a RasterContents;

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

    /// The texture of each tile's or image source's image, by the id of
    /// the tile's contents or of the source's bucket.
    textures: HashMap<SimpleIdentity, SharedTexture>,
    /// The quads of each tile's mask, by the id of its contents.
    mask_quads: MaskQuads,
    /// The contents of the frame's tiles, whose textures and quads stay.
    live_contents: HashSet<SimpleIdentity>,
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
            mask_quads: MaskQuads::default(),
            live_contents: HashSet::default(),
        }
    }

    /// The texture of `image`, of the contents or bucket `id`, in
    /// `textures`, which this creates if needed, with `sampler`.
    fn texture(
        textures: &mut HashMap<SimpleIdentity, SharedTexture>,
        sampler: SamplerState,
        context: &mut Context,
        id: SimpleIdentity,
        image: &PremultipliedImage,
    ) -> SharedTexture {
        let texture = textures
            .entry(id)
            .or_insert_with(|| context.add_texture_with(|texture| texture.set_image(Some(Rc::new(image.clone())))))
            .clone();
        if let Some(mut texture) = context.texture_mut(&texture) {
            texture.set_sampler(sampler);
        }
        texture
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
        let Some((bucket, quads)) = image_data.bucket.and_then(|bucket| Some((bucket, bucket.quads()?))) else {
            objects.images.clear();
            objects.image = None;
            return;
        };
        let built_of = (bucket.id(), bucket.generation());
        if objects.image != Some(built_of) {
            objects.images.clear();
            objects.image = Some(built_of);
            // Only the image's bucket draws: the textures of the images
            // before it go.
            textures.retain(|id, _| *id == bucket.id());
        }
        let texture = Self::texture(textures, sampler, paint_parameters.context, bucket.id(), &bucket.image);
        let tile_ids = image_data.tile_ids;
        objects.images.truncate(tile_ids.len());
        while objects.images.len() < tile_ids.len() {
            let draw_geometry =
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments);
            let color_mode = ColorMode::alpha_blended();
            objects.images.push(KeptDraw::new(new_objects(&raster_shader, color_mode, draw_geometry, &texture)));
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
            writer.uniform(PROPS_ID, &props);
            writer.storage(DRAWABLE_ID, |(draw_ubo, _)| *draw_ubo);
            writer.storage(PROJECTION_ID, |(_, projection)| *projection);
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
    let spin = mln_style::math::deg2radf(spin);
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
    texture: &SharedTexture,
) -> DrawObjects {
    let mut textures = Textures::default();
    textures[IMAGE0_SLOT] = Some(texture.clone());
    textures[IMAGE1_SLOT] = Some(texture.clone());
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
        "pos",
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a1) as u32,
            stride,
        ),
    );
    attrs.set_shared(
        "texture_pos",
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

    fn blends(&self, evaluated: &EvaluatedLayer<Raster>) -> bool {
        *evaluated.paint.raster_layer_blend.uniform() != LayerBlendType::Normal
    }

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
    /// it builds for a tile it keeps none of or whose mask changed since,
    /// and writes the layer's buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Raster>],
    ) {
        let sampler = self.sampler_state();
        let RenderRasterLayer { base, drawing, draw_list, static_geometry, globe_meshes, textures, mask_quads, .. } =
            self;
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

            let Some(raster) = tile.bucket() else {
                *entry.value = None;
                continue;
            };

            // A draw of other contents, or of the contents before their mask
            // changed, is built again.
            entry.set_bucket(raster.id());
            let geometry_generation = raster.masking.generation();
            if entry.value.as_ref().is_some_and(|cached| cached.geometry_generation != geometry_generation) {
                *entry.value = None;
            }

            if entry.value.is_none() {
                let context = &mut *paint_parameters.context;
                let texture = Self::texture(textures, sampler, context, raster.id(), &raster.image);

                // Only a masked tile has quads; any other tile is drawn with
                // the static quad over the tile's extent. On the globe, the
                // draw is the pole-capped grid of what the mask keeps.
                let mask = raster.masking.mask();
                let draw_geometry = if base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh = globe_meshes.get(context, &tile_id.canonical, mask, RasterLayoutVertex::new);
                    raster_geometry(
                        &globe_mesh.vertices,
                        globe_mesh.vertices.count(),
                        globe_mesh.indexes(),
                        &globe_mesh.segments,
                    )
                } else if let Some(quads) = mask_quads.get(context, raster.id(), &raster.masking) {
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
                let color_mode = ColorMode::alpha_blended();
                let objects = new_objects(&raster_shader, color_mode, draw_geometry, &texture);
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

    /// Drops the textures and quads of the tiles the frame doesn't draw,
    /// in every frame, including one the layer draws nothing in. The frame's
    /// draws and its drapes' share the others. An image source's draws keep
    /// their image's texture themselves.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, Raster>]) {
        if params.source.is_some_and(|source| source.image_render_data().is_some()) {
            return;
        }
        let live_contents = &mut self.live_contents;
        live_contents.clear();
        live_contents.extend(tiles.iter().filter_map(|tile| Some(tile.bucket()?.id())));
        self.textures.retain(|id, _| live_contents.contains(id));
        self.mask_quads.retain(live_contents);
    }

    fn release(&mut self) {
        self.textures.clear();
        self.mask_quads.clear();
    }
}

impl RenderLayer for RenderRasterLayer {}
