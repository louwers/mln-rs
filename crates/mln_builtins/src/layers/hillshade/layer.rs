// Each DEM tile is first drawn into a target of its own by the prepare
// shader (slopes from the DEM), then the tile's draw, with the quads of the
// tile's mask, shades that texture. The tile's value keeps the target, with
// the id of the tile's contents and the generation of the DEM's border it
// was made at, which the tile's next value takes while they are the same.
// The slopes depend on nothing but the DEM and its tile's zoom, so the layer
// draws into each target until it holds them (`Target::holds`): once, unless
// the frame that drew them failed.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_engine::draw::globe::GlobeTileMeshCache;
use mln_engine::draw::{
    Context, Draw, DrawGeometry, DrawIndexes, Draws, Indexes, LayerFrame, Mesh, RasterLayoutVertex, RenderStaticData,
    Resources, SegmentBase, SharedAttribute, SharedIndexes, SharedTexture, SharedVertices, SimpleIdentity, StoredQuads,
    Target, Textures, Uniforms, VertexAttributeArray, cast_matrix, projection_data, to_projection_ubo,
};
use mln_engine::layer::{
    DemContents, EvaluateCx, EvaluatedLayer, Evaluation, FrameTile, Layer, LayerValues, OnTerrain, PrepareCx,
    SurfaceLight, TileCx, TileData, UpdateCx, copied_vertices,
};
use mln_engine::shaders::terrain::ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES};
use mln_engine::source::DemTiles;
use mln_engine::tile::UnwrappedTileID;
use mln_engine::tile::dem::{DemUnpack, TERRAIN_RGB_MAXZOOM};
use mln_gfx::gpu::ShaderKey;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, Rgba, Size};
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::layers::Hillshade;
use mln_style::layers::hillshade_layer_properties::HillshadePaintProperties;
use mln_style::math::deg2radf;
use mln_style::types::{HillshadeIlluminationAnchorType, HillshadeMethodType, ResamplingType, TranslateAnchorType};

use crate::layers::hillshade::programs;
use crate::layers::hillshade::ubo::{HillshadeDrawableUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};

/// The slots of the programs' textures that the draws bind.
const IMAGE_SLOT: usize = programs::HILLSHADE.texture_slot("image");

/// What the layer keeps of a tile: the draw that shades its DEM's slopes,
/// and the target they are prepared in.
pub(crate) struct HillshadeTile {
    mesh: Mesh,
    dem: DemTarget,
}

/// What the layer keeps of a tile's DEM: the target its slopes are
/// prepared in, and the draw that prepares them.
struct DemTarget {
    /// The id of the tile's contents.
    id: SimpleIdentity,
    target: Target,
    /// The DEM, which `prepare` samples.
    _dem: SharedTexture,
    /// The mesh of the draw that prepares the slopes, and the blocks it
    /// reads.
    prepare: Mesh,
    blocks: (HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO),
    /// The generation of the DEM's border the target was made at.
    generation: u64,
}

pub(crate) struct RenderHillshadeLayer {
    maxzoom: u8,

    static_data_shared_vertices: Option<SharedVertices>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<SharedIndexes>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The illumination of the evaluation the draws last drew with, before
    /// a viewport anchor turns it by the map's bearing.
    illumination: Option<(Arc<EvaluatedLayer<Hillshade>>, Illumination)>,
}

/// Structure to hold processed illumination properties
struct IlluminationProperties {
    direction_radians: Vec<f32>,
    altitude_radians: Vec<f32>,
    shadow_colors: Vec<Color>,
    highlight_colors: Vec<Color>,
}

/// The packed illumination of an evaluation, and its number of light
/// sources.
#[derive(Clone, Copy)]
struct Illumination {
    props: HillshadeEvaluatedPropsUBO,
    num_lights: usize,
}

/// Pad shorter arrays by repeating the last element.
fn pad_array<T: Clone + Default>(arr: &mut Vec<T>, max_length: usize) {
    let last = arr.last().cloned().unwrap_or_default();
    let length = max_length.max(arr.len()).max(1);
    arr.resize(length, last);
}

/// Convert style properties to illumination properties
fn illumination_properties(evaluated: &HillshadePaintProperties::PossiblyEvaluated) -> IlluminationProperties {
    // Get the values from evaluated properties (these are now vectors)
    let mut directions: Vec<f32> = evaluated.hillshade_illumination_direction.uniform().clone();
    let mut altitudes: Vec<f32> = evaluated.hillshade_illumination_altitude.uniform().clone();
    let mut highlights: Vec<Color> = evaluated.hillshade_highlight_color.uniform().clone();
    let mut shadows: Vec<Color> = evaluated.hillshade_shadow_color.uniform().clone();

    // Find the maximum length to ensure all arrays are the same size
    let mut max_length = directions.len().max(altitudes.len()).max(highlights.len()).max(shadows.len());

    // Ensure we don't exceed the maximum supported
    max_length = max_length.min(MAX_ILLUMINATION_SOURCES);

    pad_array(&mut directions, max_length);
    pad_array(&mut altitudes, max_length);
    pad_array(&mut highlights, max_length);
    pad_array(&mut shadows, max_length);

    // Convert degrees to radians
    IlluminationProperties {
        direction_radians: directions.iter().map(|&deg| deg2radf(deg)).collect(),
        altitude_radians: altitudes.iter().map(|&deg| deg2radf(deg)).collect(),
        shadow_colors: shadows,
        highlight_colors: highlights,
    }
}

/// Pack illumination properties into the UBO format
fn pack_evaluated_props(illumination: &IlluminationProperties, accent_color: &Color) -> HillshadeEvaluatedPropsUBO {
    let mut ubo = HillshadeEvaluatedPropsUBO {
        // Pack accent color
        accent: [accent_color.r, accent_color.g, accent_color.b, accent_color.a],
        ..HillshadeEvaluatedPropsUBO::default()
    };

    // Pack altitudes (up to 4)
    for (i, &altitude) in illumination.altitude_radians.iter().take(4).enumerate() {
        ubo.altitudes[i] = altitude;
    }

    // Pack azimuths (up to 4)
    for (i, &azimuth) in illumination.direction_radians.iter().take(4).enumerate() {
        ubo.azimuths[i] = azimuth;
    }

    // Pack shadow and highlight colors (up to 4)
    for (shadow, color) in ubo.shadows.iter_mut().zip(&illumination.shadow_colors) {
        *shadow = [color.r, color.g, color.b, color.a];
    }
    for (highlight, color) in ubo.highlights.iter_mut().zip(&illumination.highlight_colors) {
        *highlight = [color.r, color.g, color.b, color.a];
    }

    ubo
}

/// Convert `HillshadeMethodType` enum to int for shader
fn method_to_int(method: HillshadeMethodType) -> i32 {
    method as i32
}

/// The vertex attributes of a hillshade (or prepare) draw over
/// `vertices`.
fn hillshade_vertex_attributes(vertices: &SharedVertices) -> VertexAttributeArray {
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

/// The static data's quad, its vertices and its triangles, which this adds
/// to `context`'s stores the first time.
fn static_quad<'a>(
    vertices: &'a mut Option<SharedVertices>,
    indexes: &'a mut Option<SharedIndexes>,
    context: &mut Context,
) -> (&'a SharedVertices, &'a SharedIndexes) {
    (
        vertices.get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices())),
        indexes.get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices()))),
    )
}

/// The target of the slopes of `dem`, the DEM of the tile `tile_id`, of a
/// source of `maxzoom`: `previous`, if that was made of the same DEM at its
/// border's generation, or a new one, drawn over `quad`, the static data's
/// ([`static_quad`]).
fn dem_target(
    previous: Option<DemTarget>,
    res: &mut Resources<'_>,
    dem: &DemContents,
    tile_id: &UnwrappedTileID,
    maxzoom: u8,
    (vertices, indexes): (&SharedVertices, &SharedIndexes),
) -> DemTarget {
    match previous {
        Some(previous) if previous.id == dem.id() && previous.generation == dem.generation() => previous,
        _ => {
            let shader =
                res.context.shader_key(&programs::HILLSHADE_PREPARE, Uniforms::default(), ProjectionVariant::Mercator);
            let attrs = hillshade_vertex_attributes(vertices);
            DemTarget::new(res, dem, tile_id, maxzoom, &shader, attrs, indexes)
        }
    }
}

/// Whether the light of the layer's values `evaluated` is anchored to the
/// viewport, so that it turns with the map's bearing.
fn lit_by_bearing(evaluated: &EvaluatedLayer<Hillshade>) -> bool {
    *evaluated.paint.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport
}

impl Layer for RenderHillshadeLayer {
    type Style = Hillshade;
    type Input = DemTiles;
    type Tile = HillshadeTile;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;
    // A tile's draw holds its mask's geometry and samples its DEM's render
    // target, whose sampler each frame sets: nothing of the style's values.
    const RESTYLE_KEEPS_TILES: bool = true;

    fn new(_values: &LayerValues<Hillshade>) -> Self {
        RenderHillshadeLayer {
            maxzoom: TERRAIN_RGB_MAXZOOM,
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            illumination: None,
        }
    }

    /// Lit from the viewport, the surface shades its slopes with its lights
    /// at the frame's bearing, so that its drapes don't turn with the map.
    fn surface_light(&mut self, values: &LayerValues<Hillshade>, frame: &LayerFrame<'_>) -> Option<SurfaceLight> {
        let evaluated = &values.evaluated;
        if !lit_by_bearing(evaluated) {
            return None;
        }
        let Illumination { props, num_lights } = Self::illumination(&mut self.illumination, evaluated, frame);
        let paint = &evaluated.paint;
        let tile = HillshadeTilePropsUBO {
            exaggeration: *paint.hillshade_exaggeration.uniform(),
            method: method_to_int(*paint.hillshade_method.uniform()),
            num_lights: num_lights as i32,
            ..HillshadeTilePropsUBO::default()
        };
        Some(SurfaceLight { props, tile })
    }

    /// The layer draws a drape by the map's bearing where its light is
    /// anchored to the viewport.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let evaluated = &*cx.evaluated;
        let evaluation = if *evaluated.paint.hillshade_exaggeration.uniform() > 0.0 {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        };
        Evaluation { drapes_by_bearing: lit_by_bearing(evaluated), ..evaluation }
    }

    /// The draw that shades the slopes of a tile's DEM, with the DEM's
    /// target of the tile's previous value where that is of the same DEM,
    /// or a new one; none for a tile without a DEM to draw.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, RenderHillshadeLayer>,
        dem: TileData<'_, RenderHillshadeLayer>,
        previous: Option<HillshadeTile>,
    ) -> Option<HillshadeTile> {
        if !dem.has_data() {
            return None;
        }
        let RenderHillshadeLayer { maxzoom, static_data_shared_vertices, static_data_indices, globe_meshes, .. } = self;
        let context = &mut *cx.res.context;
        let hillshade_shader =
            context.shader_key(&programs::HILLSHADE, Uniforms::default(), cx.base.projection_variant);
        let quad = static_quad(static_data_shared_vertices, static_data_indices, context);
        let tile_id = tile.draw_id();
        let previous = previous.map(|previous| previous.dem);
        let target = dem_target(previous, &mut cx.res, dem, &tile_id.to_unwrapped(), *maxzoom, quad);
        let target_texture = target.target.texture();
        let context = &mut *cx.res.context;

        let masking = &dem.masking;
        // The tile's geometry: the globe's mesh of its mask, the mask's
        // quads, or the whole tile's quad.
        let (vertices, vertex_count, indexes, segments): (SharedVertices, usize, DrawIndexes, Vec<SegmentBase>) =
            if cx.base.projection_variant == ProjectionVariant::Globe {
                let globe_mesh = globe_meshes.get(context, &tile_id.canonical, masking.mask(), RasterLayoutVertex::new);
                (
                    globe_mesh.vertices.clone(),
                    globe_mesh.vertices.count(),
                    globe_mesh.indexes(),
                    globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                )
            } else if let Some(quads) = StoredQuads::of_mask(context, masking) {
                (
                    quads.vertices.clone(),
                    quads.vertices.count(),
                    quads.indexes.clone().into(),
                    quads.segments.iter().map(SegmentBase::copy).collect(),
                )
            } else {
                let (vertices, indexes) = quad;
                (vertices.clone(), 4, indexes.clone().into(), RenderStaticData::raster_segments())
            };
        let geometry = DrawGeometry {
            vertices: hillshade_vertex_attributes(&vertices),
            vertex_count: if segments.is_empty() { 0 } else { vertex_count },
            mode: DrawMode::Triangles(),
            indexes,
            segments,
            instances: None,
        };
        let mut textures = Textures::default();
        textures[IMAGE_SLOT] = Some(target_texture.clone());
        let mesh =
            cx.res.mesh(hillshade_shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, textures);
        Some(HillshadeTile { mesh, dem: target })
    }

    /// Pushes the draw that prepares the slopes into each target of the
    /// tiles' values that doesn't hold them, and
    /// pushes the frame's draws of the layer, one of each of its tiles,
    /// which shade the slopes. In a drape that the surface shades
    /// ([`LayerFrame::drape_lit`]), they draw the slopes as they were
    /// prepared, without lights.
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderHillshadeLayer>],
        out: &mut Draws,
    ) {
        if cx.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
        // `resampling` filters the prepared texture the tile's draw samples;
        // the prepare pass reads the DEM with Nearest regardless.
        let resampling_filter = match *cx.values.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };

        // The illumination, which the first draw computes.
        let mut illumination = None;
        for tile in render_tiles {
            let tile_id = tile.draw_id();
            if !tile.data().is_some_and(DemContents::has_data) {
                cx.tiles.drop_value(&tile.id());
                continue;
            }
            let Some(HillshadeTile { mesh: kept, dem: target }) = cx.tiles.get_mut(&tile.id()) else {
                continue;
            };
            if !target.target.holds(&cx.res) {
                out.target(&target.target).push(&target.prepare, Draw::DEFAULT, target.blocks);
            }
            let context = &mut *cx.res.context;
            let target_texture = target.target.texture();
            if let Some(mut texture) = context.texture_mut(target_texture) {
                texture.set_sampler(SamplerState {
                    filter: resampling_filter,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            let draw = Draw { order: kept.seq(), ..Draw::DEFAULT };
            let Illumination { num_lights, .. } = *illumination
                .get_or_insert_with(|| Self::illumination(&mut self.illumination, &cx.values.evaluated, cx.frame));
            let num_lights = if cx.frame.drape_lit() { 0 } else { num_lights };
            let (blocks, projection) =
                Self::uniforms(&cx.values.evaluated.paint, &tile_id.to_unwrapped(), num_lights, cx.frame);
            out.main().push_at(kept, tile_id, &projection, draw, blocks);
        }

        if let Some(Illumination { props, .. }) = &illumination {
            out.main().block(props);
        }
    }

    /// Keeps the source's maximum zoom, which the DEMs' preparation reads.
    fn update(&mut self, cx: UpdateCx<'_, '_, Self>, _tiles: &[FrameTile<'_, RenderHillshadeLayer>]) {
        if let Some(max_zoom) = cx.source_max_zoom() {
            self.maxzoom = max_zoom;
        }
    }
}

impl DemTarget {
    /// The target of the slopes of `dem`, which the draw of `shader`
    /// prepares from it, drawn over `attrs` with the `quad` of the
    /// static data, with the uniforms of `tile_id`, the tile it is made for,
    /// of a source of `maxzoom`.
    fn new(
        res: &mut Resources<'_>,
        dem: &DemContents,
        tile_id: &UnwrappedTileID,
        maxzoom: u8,
        shader: &ShaderKey,
        attrs: VertexAttributeArray,
        indexes: &SharedIndexes,
    ) -> DemTarget {
        let (dem_contents_id, generation) = (dem.id(), dem.generation());
        let (tilesize, stride, encoding, dem_image) = {
            let dem = dem.dem();
            (dem.dim as u16, dem.stride, dem.encoding, dem.image().clone())
        };
        // The draw into the target covers every texel, unblended.
        let size = Size::new(u32::from(tilesize), u32::from(tilesize));
        let target = res.target(size, TextureChannelDataType::UnsignedByte, Rgba::TRANSPARENT);

        // Nearest, as the Sobel kernel samples exact texel values.
        let dem = res.context.add_texture_with(|texture| {
            texture.set_image(Some(Rc::new(dem_image)));
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Nearest,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        });
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: 4,
            mode: DrawMode::Triangles(),
            indexes: indexes.clone().into(),
            segments: RenderStaticData::raster_segments(),
            instances: None,
        };
        let mut textures = Textures::default();
        textures[IMAGE_SLOT] = Some(dem.clone());
        let prepare = res.mesh(shader.clone(), ColorMode::unblended(), CullFaceMode::disabled(), geometry, textures);
        let extent = f64::from(EXTENT);
        let m =
            orthographic(0.0, extent, -extent, 0.0, -1.0, 1.0) * DMat4::from_translation(DVec3::new(0.0, -extent, 0.0));
        let draw_ubo = HillshadePrepareDrawableUBO { matrix: cast_matrix(&m) };
        let unpack = DemUnpack::of(encoding);
        let tile_props = HillshadePrepareTilePropsUBO {
            unpack: unpack.weights_and_base(),
            dimension: [stride as f32, stride as f32],
            zoom: f32::from(tile_id.canonical.z),
            maxzoom: f32::from(maxzoom),
            unpack_scale: unpack.scale,
            ..HillshadePrepareTilePropsUBO::default()
        };
        DemTarget { id: dem_contents_id, target, _dem: dem, prepare, blocks: (draw_ubo, tile_props), generation }
    }
}

impl RenderHillshadeLayer {
    /// The blocks and the projection of the draw of `tile_id` with the
    /// layer's values `evaluated`, lit by `num_lights` light sources.
    fn uniforms(
        evaluated: &HillshadePaintProperties::PossiblyEvaluated,
        tile_id: &UnwrappedTileID,
        num_lights: usize,
        frame: &LayerFrame<'_>,
    ) -> ((HillshadeDrawableUBO, HillshadeTilePropsUBO), ProjectionUBO) {
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
        let drawable = HillshadeDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) };
        let tile_props = HillshadeTilePropsUBO {
            latrange: mln_engine::geo::lat_range(tile_id),
            exaggeration: *evaluated.hillshade_exaggeration.uniform(),
            method: method_to_int(*evaluated.hillshade_method.uniform()),
            num_lights: num_lights as i32,
            pad0: 0.0,
            pad1: 0.0,
            pad2: 0.0,
        };
        ((drawable, tile_props), to_projection_ubo(&projection))
    }

    /// The illumination of the layer's evaluation `layer` at `frame`'s
    /// bearing, which a viewport anchor turns its lights by. The
    /// evaluation's own is kept in `cache`, so that a turning map doesn't
    /// compute it again.
    fn illumination(
        cache: &mut Option<(Arc<EvaluatedLayer<Hillshade>>, Illumination)>,
        layer: &Arc<EvaluatedLayer<Hillshade>>,
        frame: &LayerFrame<'_>,
    ) -> Illumination {
        let evaluated = &layer.paint;
        let mut illumination = match cache {
            Some((cached_layer, illumination)) if Arc::ptr_eq(cached_layer, layer) => *illumination,
            _ => {
                let properties = illumination_properties(evaluated);
                let props = pack_evaluated_props(&properties, evaluated.hillshade_accent_color.uniform());
                let illumination = Illumination { props, num_lights: properties.direction_radians.len() };
                *cache = Some((Arc::clone(layer), illumination));
                illumination
            }
        };
        if *evaluated.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport {
            let bearing = frame.view.angle() as f32;
            let lights = illumination.num_lights.min(illumination.props.azimuths.len());
            for azimuth in &mut illumination.props.azimuths[..lights] {
                *azimuth -= bearing;
            }
        }
        illumination
    }
}
