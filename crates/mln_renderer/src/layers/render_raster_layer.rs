// The layer keeps one texture per bucket id (`textures`), in the context's
// store: buckets are Send and cross from the workers without one. A tile's
// draw stays until the bucket's masked geometry changes, and is then built
// again.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
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
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr};
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
use crate::gfx::vertex_attribute::VertexAttributeArray;
use crate::gfx::vertices::SharedVertices;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_composite::Composite;
use crate::layer_drawing::{
    DrawGroup, DrawList, DrawObjectsMut, DrawOrder, FrameDraw, KeptDraw, LayerDrawing, LayerDraws, TileMap,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{Atlases, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::sources::render_image_source::ImageSourceRenderData;
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

    fn upload(&mut self, context: &mut Context) {
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

    raster_shader: Option<ShaderProgramPtr>,

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
    /// The draws of an image source's layer: one for each tile the image is
    /// drawn in, built again every frame, which share the image's geometry
    /// and texture.
    fn update_image(
        &mut self,
        raster_shader: &ShaderProgramPtr,
        image_data: ImageSourceRenderData<'_>,
        paint_parameters: &mut PaintParameters<'_>,
    ) {
        self.drawing.objects.images.clear();

        // The bucket is missing until the image loads.
        let Some(bucket) = image_data.bucket else {
            return;
        };
        // The bucket's own geometry, which an image source's bucket always
        // has.
        let geometry = bucket.geometry();
        let Some(quads) = geometry.quads() else {
            return;
        };

        let sampler = self.sampler_state();
        let texture = Self::bucket_texture(&mut self.textures, sampler, paint_parameters.context, bucket);
        let image_draws = &mut self.drawing.objects.images;
        for (index, tile_id) in image_data.tile_ids.iter().enumerate() {
            let draw_geometry =
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments);
            let draw = KeptDraw::new(new_objects(raster_shader, draw_geometry, texture.as_ref()));
            let uniforms = Self::uniforms(tile_id, paint_parameters);
            self.draw_list.push_part(FrameDraw::new(None, draw.id), RasterPart::Image(index), uniforms);
            image_draws.push(draw);
        }

        self.finish_frame_draws(paint_parameters.context);
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
            !parameters.state.is_changing(),
        );
        (RasterDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) }, to_projection_ubo(&projection))
    }

    /// Makes the draws of the draw list the layer's draws of the frame, and
    /// writes the layer's buffers of them.
    fn finish_frame_draws(&mut self, context: &mut Context) {
        let composite = self.composite();
        let Some(frame) = self.drawing.frame.as_mut() else {
            return;
        };
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
fn new_objects(shader: &ShaderProgramPtr, geometry: DrawGeometry, texture: Option<&SharedTexture>) -> DrawObjects {
    let mut textures = Textures::default();
    textures[ID_RASTER_IMAGE0_TEXTURE] = texture.cloned();
    textures[ID_RASTER_IMAGE1_TEXTURE] = texture.cloned();
    DrawObjects::new(Rc::clone(shader), ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, textures)
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
    attrs.share(
        ID_RASTER_POS_VERTEX_ATTRIBUTE,
        vertices.clone(),
        offset_of!(RasterLayoutVertex, a1) as u32,
        0,
        stride,
        AttributeDataType::Short2,
    );
    attrs.share(
        ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
        vertices.clone(),
        offset_of!(RasterLayoutVertex, a2) as u32,
        0,
        stride,
        AttributeDataType::Short2,
    );
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

    /// Drops the shader of the last projection, and the globe's meshes
    /// when the frame isn't on the globe.
    fn projection_changed(&mut self) {
        self.raster_shader = None;
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
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Raster>],
    ) {
        let Some(raster_shader) = self.raster_shader(paint_parameters) else {
            return;
        };

        let sampler = self.sampler_state();
        let static_geometry = self.static_geometry.get_or_insert_with(|| StaticGeometry::new(paint_parameters.context));

        // Drop the textures of buckets that are no longer rendered.
        let live_buckets = &mut self.live_buckets;
        live_buckets.clear();
        live_buckets.extend(render_tiles.iter().filter_map(|tile| Some(tile.bucket()?.base.bucket_id)));
        self.textures.retain(|id, _| live_buckets.contains(id));
        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        for texture in self.textures.values() {
            if let Some(mut texture) = paint_parameters.context.texture_mut(texture) {
                texture.set_sampler(sampler);
            }
        }

        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.objects.tiles.entry(&tile_id) else {
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
                let texture = Self::bucket_texture(&mut self.textures, sampler, paint_parameters.context, bucket);

                // The bucket only has geometry for a masked tile; any other
                // tile is drawn with the static quad over the tile's extent.
                // On the globe, the draw is the pole-capped grid of what the
                // mask keeps.
                let geometry = bucket.geometry();
                let context = &mut *paint_parameters.context;
                let draw_geometry = if self.base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        self.globe_meshes.get(context, &tile_id.canonical, &geometry.mask, RasterLayoutVertex::new);
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
                let objects = new_objects(&raster_shader, draw_geometry, texture.as_ref());
                let built = RasterTile { draw: KeptDraw::new(objects), geometry_generation };
                *entry.value = Some(built);
            }

            if let Some(cached) = entry.value.as_ref() {
                let draw = cached.draw.frame_draw(Some(tile_id));
                let uniforms = Self::uniforms(&tile_id.to_unwrapped(), paint_parameters);
                self.draw_list.push_part(draw, RasterPart::Tile, uniforms);
            }
        }

        self.finish_frame_draws(paint_parameters.context);
    }

    fn update_image_draws(
        &mut self,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        image: ImageSourceRenderData<'_>,
    ) {
        if let Some(raster_shader) = self.raster_shader(paint_parameters) {
            self.update_image(&raster_shader, image, paint_parameters);
        }
    }

    fn release(&mut self) {
        self.textures.clear();
    }
}

impl RenderRasterLayer {
    /// The shader of the layer's draws in the frame's projection, made on
    /// first use.
    fn raster_shader(&mut self, paint_parameters: &PaintParameters<'_>) -> Option<ShaderProgramPtr> {
        if self.raster_shader.is_none() {
            let shaders = &paint_parameters.static_data.shaders;
            self.raster_shader = shaders.shader("RasterShader", self.base.projection_variant);
        }
        self.raster_shader.clone()
    }
}

impl RenderLayer for RenderRasterLayer {}
