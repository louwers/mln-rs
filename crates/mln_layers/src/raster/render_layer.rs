// The layer keeps one texture per contents id (`textures`), in the context's
// store: the image crosses from the workers without one. A tile's
// draw stays until the contents' masked geometry changes, and is then built
// again.

use std::mem::offset_of;
use std::rc::Rc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_render::draw::{
    Composite, Context, DrawFrame, DrawGeometry, DrawGroup, DrawIndexes, DrawList, DrawObjects, DrawOrder, FrameDraw,
    FrameDraws, Indexes, LayerDrawing, Mesh, PaintParameters, RenderStaticData, SharedAttribute, SharedIndexes,
    SharedTexture, SharedVertices, Textures, TileMap, Uniforms, VertexAttributeArray, cast_matrix, copied_vertices,
    projection_data, to_projection_ubo,
};
use mln_render::layer::{
    DrawContext, EvaluateCx, Evaluation, LayerType, LayerValues, NoBucket, NoLayout, RasterContents,
    RasterLayoutVertex, RasterView, UpdateContext,
};
use mln_style_types::{RasterResamplingType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;

use super::programs::{
    DRAWABLE, ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, PROPS,
};
use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{Raster, programs};
use crate::mesh::GlobeTileMeshCache;

/// What the layer keeps of a tile: its draw, and the generation of the
/// contents' geometry it was built of.
pub struct RasterTile {
    mesh: Mesh,
    geometry_generation: u64,
}

/// What the layer keeps of the draws of an image source's image: one for
/// each tile it is drawn in, with the image's contents and the generation of
/// its geometry they were built of.
#[derive(Default)]
struct RasterObjects {
    images: Vec<Mesh>,
    image: Option<(SimpleIdentity, u64)>,
}

/// The layer's drawing: the meshes of an image source's draws, and its
/// draws of the frame.
#[derive(Default)]
pub struct RasterDrawing {
    objects: RasterObjects,
    frame: Option<FrameDraws>,
}

impl LayerDrawing for RasterDrawing {
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(frame));
        }
    }

    fn clear(&mut self) {
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
            frame.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(gpu, parameters);
            order.step(parameters);
        }
    }
}

/// The tile extent's geometry, which a tile draws whose contents don't
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
            segments: RenderStaticData::quad_triangle_segments(),
        }
    }
}

/// What a raster layer keeps besides its drawing.
pub struct RasterState {
    /// The frame's draws as the update makes them, with their uniform blocks
    /// and projections.
    draw_list: DrawList<(RasterDrawableUBO, ProjectionUBO)>,

    static_geometry: Option<StaticGeometry>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The texture of each contents' image, by contents id.
    textures: HashMap<SimpleIdentity, SharedTexture>,
    /// The contents of the frame's tiles, whose textures stay.
    live_contents: HashSet<SimpleIdentity>,
}

impl RasterState {
    /// The contents' texture in `textures`, which this creates if needed,
    /// with `sampler`; `None` for contents without an image.
    fn contents_texture(
        textures: &mut HashMap<SimpleIdentity, SharedTexture>,
        sampler: SamplerState,
        context: &mut Context,
        contents: &RasterContents,
    ) -> Option<SharedTexture> {
        let image = contents.image()?;
        let texture = textures
            .entry(contents.id())
            .or_insert_with(|| context.add_texture_with(|texture| texture.set_image(Some(Rc::new((**image).clone())))))
            .clone();
        if let Some(mut texture) = context.texture_mut(&texture) {
            texture.set_sampler(sampler);
        }
        Some(texture)
    }

    /// The draws of `image`, an image source's image, in each of the
    /// context's tiles, which share the image's geometry and texture, built
    /// again when the image or its geometry changes.
    fn update_image(
        &mut self,
        drawing: &mut RasterDrawing,
        gpu: &mut dyn Gpu,
        contents: &RasterContents,
        cx: &mut DrawContext<'_, '_, Raster>,
    ) {
        let layer = cx.layer;
        let sampler = sampler_state(layer);
        let RasterState { draw_list, textures, .. } = self;
        let raster_shader = cx.context.shader_key(&programs::RASTER, Uniforms::default(), cx.projection_variant);
        let objects = &mut drawing.objects;
        // An image source's contents always have geometry of their own once
        // they have an image.
        let Some(quads) = contents.geometry().quads() else {
            objects.images.clear();
            objects.image = None;
            return;
        };
        let built_of = (contents.id(), contents.geometry().generation());
        if objects.image != Some(built_of) {
            objects.images.clear();
            objects.image = Some(built_of);
            // Only the image's contents draw: the textures of the images
            // before it go.
            textures.retain(|id, _| *id == contents.id());
        }
        let texture = Self::contents_texture(textures, sampler, cx.context, contents);
        let tiles = cx.tiles;
        objects.images.truncate(tiles.len());
        while objects.images.len() < tiles.len() {
            let draw_geometry =
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments);
            let color_mode = cx.context.color_mode(ColorMode::alpha_blended());
            let image = new_objects(&raster_shader, color_mode, draw_geometry, texture.as_ref());
            objects.images.push(cx.context.add_mesh(image));
        }
        for (tile, mesh) in tiles.iter().zip(&objects.images) {
            let uniforms = Self::uniforms(&tile.id().to_unwrapped(), &cx.frame);
            draw_list.push(FrameDraw::new(None, mesh), uniforms);
        }

        self.finish_frame_draws(drawing, layer, gpu, cx.context);
    }

    /// The entries in the layer's storage buffers of a draw of `tile_id`.
    fn uniforms(tile_id: &UnwrappedTileID, frame: &DrawFrame<'_>) -> (RasterDrawableUBO, ProjectionUBO) {
        let projection = projection_data(
            tile_id,
            frame,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            !frame.camera_moving,
        );
        (RasterDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) }, to_projection_ubo(&projection))
    }

    /// Makes the draws of the draw list the layer's draws of the frame, and
    /// writes the layer's buffers of them.
    fn finish_frame_draws(
        &mut self,
        drawing: &mut RasterDrawing,
        layer: &LayerValues<Raster>,
        gpu: &mut dyn Gpu,
        context: &mut Context,
    ) {
        let evaluated = &layer.evaluated.paint;
        // The layer draws as one surface for a `raster-layer-blend` other
        // than `normal`.
        let composite = Composite::of(1.0, *evaluated.raster_layer_blend.uniform());
        let Some(frame) = drawing.frame.as_mut() else {
            return;
        };
        frame.set_composite(composite);
        self.draw_list.finish(frame, gpu, context, |writer| {
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
            writer.uniform(PROPS.id, &props);
            writer.storage(DRAWABLE.id, |(draw_ubo, _)| *draw_ubo);
            writer.storage(PROJECTION_BUFFER.id, |(_, projection)| *projection);
        });
    }
}

/// The sampler of the tile textures of a layer of `layer`'s values.
fn sampler_state(layer: &LayerValues<Raster>) -> SamplerState {
    let evaluated = &layer.evaluated.paint;
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
        mode: DrawMode::Triangles,
        indexes,
        segments: segments.to_vec(),
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

impl LayerType for Raster {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = RasterView<'a>;
    type Tile = RasterTile;
    type Drawing = RasterDrawing;
    type State = RasterState;

    fn new_layer(_layer: &LayerValues<Raster>) -> (RasterState, RasterDrawing) {
        let state = RasterState {
            draw_list: DrawList::default(),
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            live_contents: HashSet::default(),
        };
        (state, RasterDrawing::default())
    }

    /// Drops the globe's meshes when the frame isn't on the globe.
    fn projection_changed(state: &mut RasterState, variant: ProjectionVariant) {
        if variant == ProjectionVariant::Mercator {
            state.globe_meshes.clear();
        }
    }

    fn evaluate(_state: &mut RasterState, cx: EvaluateCx<'_, Raster>) -> Evaluation {
        if *cx.evaluated.paint.raster_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// Makes the frame's draws of the layer: those of an image source's
    /// image, or one of each of its tiles, which it builds for a tile it
    /// keeps none of or whose contents' geometry changed since, and writes the
    /// layer's buffers they read.
    fn make_draws(
        state: &mut RasterState,
        drawing: &mut RasterDrawing,
        tile_values: &mut TileMap<RasterTile>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Raster>,
    ) {
        let image = cx.tiles.iter().find_map(|tile| match tile.view()? {
            RasterView::Image(contents) => Some(contents),
            RasterView::Tile(_) => None,
        });
        if let Some(image) = image {
            state.update_image(drawing, gpu, image, cx);
            return;
        }
        let (render_tiles, layer) = (cx.tiles, cx.layer);
        let sampler = sampler_state(layer);
        let RasterState { draw_list, static_geometry, globe_meshes, textures, live_contents } = state;
        let raster_shader = cx.context.shader_key(&programs::RASTER, Uniforms::default(), cx.projection_variant);
        let static_geometry = static_geometry.get_or_insert_with(|| StaticGeometry::new(cx.context));

        // Drop the textures of contents that are no longer rendered.
        live_contents.clear();
        live_contents.extend(render_tiles.iter().filter_map(|tile| Some(tile.view()?.contents().id())));
        textures.retain(|id, _| live_contents.contains(id));
        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        for texture in textures.values() {
            if let Some(mut texture) = cx.context.texture_mut(texture) {
                texture.set_sampler(sampler);
            }
        }

        for (index, tile) in render_tiles.iter().enumerate() {
            let tile_id = tile.id();
            let Some(RasterView::Tile(contents)) = tile.view().filter(|view| view.contents().has_data()) else {
                continue;
            };

            // A draw of the contents' geometry before its mask changed is
            // built again.
            let value = tile_values.get_mut(index);
            let geometry_generation = contents.geometry().generation();
            if value.as_ref().is_some_and(|cached| cached.geometry_generation != geometry_generation) {
                *value = None;
            }

            if value.is_none() {
                let texture = RasterState::contents_texture(textures, sampler, cx.context, contents);

                // The contents only have geometry for a masked tile; any other
                // tile is drawn with the static quad over the tile's extent.
                // On the globe, the draw is the pole-capped grid of what the
                // mask keeps.
                let geometry = contents.geometry();
                let context = &mut *cx.context;
                let draw_geometry = if cx.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        globe_meshes.get(context, &tile_id.canonical, geometry.mask(), RasterLayoutVertex::new);
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
                let built = RasterTile { mesh: context.add_mesh(objects), geometry_generation };
                *value = Some(built);
            }

            if let Some(cached) = value.as_ref() {
                let uniforms = RasterState::uniforms(&tile_id.to_unwrapped(), &cx.frame);
                draw_list.push(FrameDraw::new(Some(tile_id), &cached.mesh), uniforms);
            }
        }

        state.finish_frame_draws(drawing, layer, gpu, cx.context);
    }

    /// A layer whose source shows it nothing keeps no textures.
    fn update(state: &mut RasterState, _drawing: &mut RasterDrawing, cx: &mut UpdateContext<'_, '_, Raster>) {
        if cx.tiles.is_empty() {
            state.textures.clear();
        }
    }

    fn release(state: &mut RasterState) {
        state.textures.clear();
    }
}
