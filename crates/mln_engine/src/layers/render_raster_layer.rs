// The layer keeps one texture and the quads of one mask per tile contents'
// id (`textures`, `mask_quads`), in the context's store, which its draws of a
// tile in the frame and in drapes share. A tile's draw stays until the
// tile's mask, or an image source's mesh, changes, and is then built again.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::ShaderKey;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::LayerStyle;
use mln_style::layers::Raster;
use mln_style::types::{RasterResamplingType, ResamplingType, TranslateAnchorType};

use super::cast_matrix;
use crate::buckets::raster_bucket::{MaskQuads, RasterLayoutVertex};
use crate::draw::context::Context;
use crate::draw::draws::{Draw, Draws, Group, LayerFrame};
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::draw::objects::{DrawGeometry, Mesh, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::store::SharedTexture;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::draw::vertices::SharedVertices;
use crate::layer::bucket_geometry::copied_vertices;
use crate::layer::layer_type::LayerType;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, Hosted, LayerValues, OnTerrain, PrepareCx, RenderLayer, TileCx, TypedLayer, UpdateCx,
};
use crate::layer::tile_input::{RasterTile, TileData};
use crate::layers::raster::programs;
use crate::layers::raster::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use crate::render::globe::globe_tile_mesh::GlobeTileMeshCache;
use crate::render::layer_composite::Composite;
use crate::render::static_data::RenderStaticData;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{projection_data, to_projection_ubo};
use crate::source::RasterTiles;
use crate::source::render_image_source::ImageQuad;
use crate::tile::{OverscaledTileID, UnwrappedTileID};

/// The slots of the programs' textures that the draws bind.
const IMAGE0_SLOT: usize = programs::RASTER.texture_slot("image0");
const IMAGE1_SLOT: usize = programs::RASTER.texture_slot("image1");

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
    type Input = RasterTiles;

    fn render_layer(style: Arc<LayerStyle<Raster>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderRasterLayer>::boxed(style)
    }
}

pub(crate) struct RenderRasterLayer {
    static_geometry: Option<StaticGeometry>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The texture of each tile's image, by its id.
    textures: HashMap<SimpleIdentity, SharedTexture>,
    /// The quads of each tile's mask, by the id of its contents.
    mask_quads: MaskQuads,
    /// The contents of the frame's tiles, whose textures and quads stay.
    live_contents: HashSet<SimpleIdentity>,
}

impl RenderRasterLayer {
    /// The texture of `image`, the image `id`, in
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

    /// How the draws of the layer of `values` composite onto the frame: as
    /// one surface, for a `raster-layer-blend` other than `normal`.
    fn composite(values: &LayerValues<Raster>) -> Option<Composite> {
        let paint = &values.evaluated.paint;
        Composite::of(1.0, *paint.raster_layer_blend.uniform())
    }

    /// The sampler of the layer's tile textures for the evaluated
    /// properties of `values`.
    fn sampler_state(values: &LayerValues<Raster>) -> SamplerState {
        let evaluated = &values.evaluated.paint;
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
    /// The block and the projection of a draw of `tile_id`.
    fn uniforms(tile_id: &UnwrappedTileID, frame: &LayerFrame<'_>) -> (RasterDrawableUBO, ProjectionUBO) {
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

    /// Gives the frame's draws of the layer of `values`, in `group`, what
    /// they share: how they composite, and their block, which puts their
    /// vertices on `quad`.
    fn share_frame_draws(values: &LayerValues<Raster>, group: &mut Group, quad: &ImageQuad) {
        let composite = Self::composite(values);
        let evaluated = &values.evaluated.paint;
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
        group.composite(composite);
        group.block(&props);
    }
}

/// Pushes the frame's draw of the tile `tile_id`, from what the layer keeps
/// of it, if anything, into `group`.
fn raster_draw(kept: Option<&Mesh>, tile_id: OverscaledTileID, frame: &LayerFrame<'_>, group: &mut Group) {
    if let Some(kept) = kept {
        let draw = Draw { order: kept.seq(), ..Draw::DEFAULT };
        let (drawable, projection) = RenderRasterLayer::uniforms(&tile_id.to_unwrapped(), frame);
        group.push_at(kept, tile_id, &projection, draw, drawable);
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

/// The mesh of a raster draw of `geometry` with `shader`, which binds
/// `texture` as both of its images.
fn new_mesh(
    context: &mut Context,
    shader: &ShaderKey,
    color_mode: ColorMode,
    geometry: DrawGeometry,
    texture: &SharedTexture,
) -> Mesh {
    let mut textures = Textures::default();
    textures[IMAGE0_SLOT] = Some(texture.clone());
    textures[IMAGE1_SLOT] = Some(texture.clone());
    Mesh::new(context, shader.clone(), color_mode, CullFaceMode::disabled(), geometry, textures)
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
    type Tile = Mesh;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn new(_values: &LayerValues<Raster>) -> Self {
        RenderRasterLayer {
            static_geometry: None,
            globe_meshes: GlobeTileMeshCache::new(),
            textures: HashMap::default(),
            mask_quads: MaskQuads::default(),
            live_contents: HashSet::default(),
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let evaluated = &*cx.evaluated;
        if *evaluated.paint.raster_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// The draw of a tile's image; none for an image source's image that
    /// has no mesh yet.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, Raster>,
        data: TileData<'_, Raster>,
    ) -> Option<Mesh> {
        let sampler = Self::sampler_state(cx.values);
        let context = &mut *cx.context;
        let raster_shader = context.shader_key(&programs::RASTER, Uniforms::default(), cx.base.projection_variant);
        let static_geometry = self.static_geometry.get_or_insert_with(|| StaticGeometry::new(context));
        let tile_id = tile.draw_id();
        let draw_geometry = match data {
            // An image source's image draws on its mesh, once it has one.
            RasterTile::Image(image) => {
                let quads = image.quads()?;
                raster_geometry(&quads.vertices, quads.vertices.count(), quads.indexes.clone().into(), &quads.segments)
            }
            // Only a masked tile has quads; any other tile is drawn with the
            // static quad over the tile's extent. On the globe, the draw is
            // the pole-capped grid of what the mask keeps.
            RasterTile::Tile(raster) => {
                let mask = raster.masking.mask();
                if cx.base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh = self.globe_meshes.get(context, &tile_id.canonical, mask, RasterLayoutVertex::new);
                    raster_geometry(
                        &globe_mesh.vertices,
                        globe_mesh.vertices.count(),
                        globe_mesh.indexes(),
                        &globe_mesh.segments,
                    )
                } else if let Some(quads) = self.mask_quads.get(context, raster.id(), &raster.masking) {
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
                }
            }
        };
        let texture = Self::texture(&mut self.textures, sampler, context, data.id(), data.image());
        let color_mode = ColorMode::alpha_blended();
        Some(new_mesh(context, &raster_shader, color_mode, draw_geometry, &texture))
    }

    /// Pushes the frame's draws of the layer, one of each of its tiles,
    /// which put the tiles' vertices on their source's quad.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, render_tiles: &[FrameTile<'_, Raster>], out: &mut Draws) {
        if cx.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
        let sampler = Self::sampler_state(cx.values);

        // Kept draws keep their textures, so a changed filter is applied to
        // the textures themselves.
        for texture in self.textures.values() {
            if let Some(mut texture) = cx.context.texture_mut(texture) {
                texture.set_sampler(sampler);
            }
        }

        for tile in render_tiles {
            let tile_id = tile.draw_id();
            raster_draw(cx.tiles.get_mut(&tile_id).map(|kept| &*kept), tile_id, cx.frame, out.main());
        }

        let quad = render_tiles.iter().find_map(|tile| tile.data()).map_or(ImageQuad::TILE, RasterTile::quad);
        Self::share_frame_draws(cx.values, out.main(), &quad);
    }

    /// Drops the textures and quads of the tiles the frame doesn't draw,
    /// in every frame, including one the layer draws nothing in. The frame's
    /// draws and its drapes' share the others.
    fn update(&mut self, _cx: UpdateCx<'_, '_, Self>, tiles: &[FrameTile<'_, Raster>]) {
        let live_contents = &mut self.live_contents;
        live_contents.clear();
        live_contents.extend(tiles.iter().filter_map(|tile| Some(tile.data()?.id())));
        self.textures.retain(|id, _| live_contents.contains(id));
        self.mask_quads.retain(live_contents);
    }

    fn release(&mut self) {
        self.textures.clear();
        self.mask_quads.clear();
    }
}
