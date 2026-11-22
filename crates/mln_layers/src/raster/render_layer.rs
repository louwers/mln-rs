// The layer keeps one texture per contents id (`textures`), in the context's
// store: the image crosses from the workers without one. A tile's
// draw stays until the contents' masked geometry changes, and is then built
// again.

use std::mem::offset_of;
use std::rc::Rc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_render::draw::{
    Composite, Context, Draw, DrawFrame, DrawGeometry, DrawIndexes, DrawObjects, Group, Indexes, Mesh,
    RenderStaticData, SharedAttribute, SharedIndexes, SharedTexture, SharedVertices, Textures, TileMap, Uniforms,
    VertexAttributeArray, copied_vertices,
};
use mln_render::layer::{
    DrawContext, EvaluateCx, Evaluation, LayerType, LayerValues, NoBucket, NoLayout, RasterContents,
    RasterLayoutVertex, RasterView, TileTransform, UpdateContext,
};
use mln_style_types::{RasterResamplingType, ResamplingType};
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;

use super::programs::{
    ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{Raster, programs};
use crate::mesh::GlobeTileMeshCache;

/// What the layer keeps of a tile: its mesh, and the generation of the
/// contents' geometry it was made of.
pub struct RasterTile {
    mesh: Mesh<RasterDrawableUBO>,
    geometry_generation: u64,
}

/// The meshes of an image source's image: one for each tile it is drawn in,
/// with the image's contents and the generation of its geometry they were
/// made of.
#[derive(Default)]
struct ImageMeshes {
    meshes: Vec<Mesh<RasterDrawableUBO>>,
    image: Option<(SimpleIdentity, u64)>,
}

impl ImageMeshes {
    fn clear(&mut self) {
        self.meshes.clear();
        self.image = None;
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

/// What a raster layer keeps.
pub struct RasterState {
    /// The meshes of an image source's image.
    images: ImageMeshes,
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

    /// Pushes the draws of `image`, an image source's image, in each of the
    /// context's tiles, whose meshes share the image's geometry and texture,
    /// made again when the image or its geometry changes.
    fn draw_image(&mut self, contents: &RasterContents, cx: &mut DrawContext<'_, '_, Raster>) {
        let layer = cx.layer;
        let sampler = sampler_state(layer);
        let RasterState { images, textures, .. } = self;
        let raster_shader = cx.context.shader_key(&programs::RASTER, Uniforms::default(), cx.projection_variant);
        // An image source's contents always have geometry of their own once
        // they have an image.
        let Some(quads) = contents.geometry().quads() else {
            images.clear();
            return;
        };
        let made_of = (contents.id(), contents.geometry().generation());
        if images.image != Some(made_of) {
            images.meshes.clear();
            images.image = Some(made_of);
            // Only the image's contents draw: the textures of the images
            // before it go.
            textures.retain(|id, _| *id == contents.id());
        }
        let texture = Self::contents_texture(textures, sampler, cx.context, contents);
        let tiles = cx.tiles;
        images.meshes.truncate(tiles.len());
        while images.meshes.len() < tiles.len() {
            let draw_geometry =
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments);
            let color_mode = cx.context.color_mode(ColorMode::alpha_blended());
            let image = new_objects(&raster_shader, color_mode, draw_geometry, texture.as_ref());
            images.meshes.push(cx.context.add_mesh(image));
        }
        let main = begin_draws(cx.draws.main(), layer);
        for (tile, mesh) in tiles.iter().zip(&images.meshes) {
            let at = tile.project(&cx.frame, &transform(&cx.frame));
            let draw = Draw { order: mesh.seq() as i64, ..Draw::DEFAULT };
            main.push_at(mesh, &at, draw, RasterDrawableUBO { matrix: at.fallback_matrix });
        }
    }
}

/// How the layer's draws place a tile's geometry: on the pixel grid while
/// the camera stands still.
fn transform(frame: &DrawFrame<'_>) -> TileTransform {
    TileTransform { aligned: !frame.camera_moving, ..TileTransform::default() }
}

/// `main`, the group of a layer of `layer`'s values, with the block of the
/// layer's values that its draws share, as one surface for a
/// `raster-layer-blend` other than `normal`.
fn begin_draws<'a>(main: &'a mut Group, layer: &LayerValues<Raster>) -> &'a mut Group {
    let evaluated = &layer.evaluated.paint;
    if let Some(composite) = Composite::of(1.0, *evaluated.raster_layer_blend.uniform()) {
        main.composite(composite);
    }
    main.block(&RasterEvaluatedPropsUBO {
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
    })
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
    type Drawing = ();
    type State = RasterState;

    fn new_layer(_layer: &LayerValues<Raster>) -> (RasterState, ()) {
        let state = RasterState {
            images: ImageMeshes::default(),
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            live_contents: HashSet::default(),
        };
        (state, ())
    }

    /// Drops the meshes of an image source's image, made for the last
    /// projection, and the globe's meshes when the frame isn't on the
    /// globe.
    fn projection_changed(state: &mut RasterState, variant: ProjectionVariant) {
        state.images.clear();
        if variant == ProjectionVariant::Mercator {
            state.globe_meshes.clear();
        }
    }

    fn evaluate(_state: &mut RasterState, cx: EvaluateCx<'_, Raster>) -> Evaluation {
        if *cx.evaluated.paint.raster_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// Pushes the frame's draws of the layer: those of an image source's
    /// image, or one of each of its tiles, whose mesh it makes for a tile it
    /// keeps none of or whose contents' geometry changed since.
    fn make_draws(
        state: &mut RasterState,
        _drawing: &mut (),
        tile_values: &mut TileMap<RasterTile>,
        _gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Raster>,
    ) {
        let image = cx.tiles.iter().find_map(|tile| match tile.view()? {
            RasterView::Image(contents) => Some(contents),
            RasterView::Tile(_) => None,
        });
        if let Some(image) = image {
            state.draw_image(image, cx);
            return;
        }
        let (render_tiles, layer) = (cx.tiles, cx.layer);
        let sampler = sampler_state(layer);
        let RasterState { static_geometry, globe_meshes, textures, live_contents, .. } = state;
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

        let main = begin_draws(cx.draws.main(), layer);
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

            if let Some(made) = value.as_ref() {
                let at = tile.project(&cx.frame, &transform(&cx.frame));
                let draw = Draw { order: made.mesh.seq() as i64, ..Draw::DEFAULT };
                main.push_at(&made.mesh, &at, draw, RasterDrawableUBO { matrix: at.fallback_matrix });
            }
        }
    }

    /// A layer whose source shows it nothing keeps no textures or meshes.
    fn update(state: &mut RasterState, _drawing: &mut (), cx: &mut UpdateContext<'_, '_, Raster>) {
        if cx.tiles.is_empty() {
            state.images.clear();
            state.textures.clear();
        }
    }

    fn release(state: &mut RasterState) {
        state.images.clear();
        state.textures.clear();
    }
}
