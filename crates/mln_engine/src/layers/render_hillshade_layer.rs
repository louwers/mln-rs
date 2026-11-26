// Each DEM tile is first drawn into its own render target by the prepare
// shader (slopes from the DEM), then the tile's draw shades that texture.
// The layer keeps the render targets and the quads of the masks per tile
// contents' id (`dem_targets`, `mask_quads`), each target with the
// generation of the DEM's border it was made at, and a tile's draw takes the
// mask's geometry again only when it changed. The slopes depend on nothing
// but the DEM and its tile's zoom, so each target is prepared once, by the
// first frame whose prepare draw, pass and submission don't fail.

use std::collections::hash_map::Entry;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, Size};
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::geo::LatLng;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::LayerStyle;
use mln_style::layers::Hillshade;
use mln_style::layers::hillshade_layer_properties::HillshadePaintProperties;
use mln_style::math::deg2radf;
use mln_style::types::{HillshadeIlluminationAnchorType, HillshadeMethodType, ResamplingType, TranslateAnchorType};

use super::cast_matrix;
use crate::buckets::raster_bucket::{MaskQuads, RasterLayoutVertex};
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::draw::objects::{DrawGeometry, DrawObjects, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::store::SharedTexture;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::draw::vertices::SharedVertices;
use crate::layer::bucket_geometry::copied_vertices;
use crate::layer::layer_drawing::{DrawList, DrawObjectsMut, FrameDraw, KeptDraw, SingleDraw, TileDraws, TileObjects};
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::layout::NoLayout;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, LayerUpdateParameters, LayerValues, OnTerrain, Parts, PartsMut, RenderLayer,
    RenderLayerBase, TypedLayer,
};
use crate::layers::hillshade::programs;
use crate::layers::hillshade::ubo::{HillshadeDrawableUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use crate::render::globe::globe_tile_mesh::GlobeTileMeshCache;
use crate::render::paint_parameters::PaintParameters;
use crate::render::static_data::RenderStaticData;
use crate::render::target::RenderTarget;
use crate::render::terrain::SurfaceLight;
use crate::render::terrain::ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES};
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{bind_no_projection, projection_data, to_projection_ubo};
use crate::source::render_source::RenderSource;
use crate::tile::UnwrappedTileID;
use crate::tile::dem::{DemUnpack, TERRAIN_RGB_MAXZOOM};
use crate::tile_store::dem::DemContents;

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::HILLSHADE.buffer_id("drawableVector");
const TILE_PROPS_ID: usize = programs::HILLSHADE.buffer_id("tilePropsVector");
const PROPS_ID: usize = programs::HILLSHADE.buffer_id("props");
const PREPARE_DRAWABLE_ID: usize = programs::HILLSHADE_PREPARE.buffer_id("drawable");
const PREPARE_TILE_PROPS_ID: usize = programs::HILLSHADE_PREPARE.buffer_id("tileProps");

/// The slots of the programs' textures that the draws bind.
const IMAGE_SLOT: usize = programs::HILLSHADE.texture_slot("image");

impl LayerType for Hillshade {
    type Layout = NoLayout;
    type TileBucket<'a> = &'a DemContents;

    fn render_layer(style: Arc<LayerStyle<Hillshade>>) -> Box<dyn RenderLayer> {
        Box::new(RenderHillshadeLayer::new(style))
    }
}

/// What the layer keeps of a tile's DEM: the render target its slopes are
/// prepared in, and the draw that prepares them.
struct DemTarget {
    render_target: RenderTarget,
    /// The DEM, which `prepare` samples.
    _dem: SharedTexture,
    /// The draw that prepares the slopes, with the uniforms it reads.
    prepare: SingleDraw,
    /// The generation of the DEM's border the target was made at.
    generation: u64,
    /// Whether the target holds the slopes, which a pass drew without
    /// failing.
    prepared: bool,
}

/// What the layer keeps of a tile: the draw that shades the slopes of its
/// DEM, with the generation of the tile's mask it took.
pub(crate) struct HillshadeTile {
    draw: KeptDraw,
    geometry_generation: u64,
}

impl TileObjects for HillshadeTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        self.draw.objects(())
    }

    fn drop_bindings(&mut self) {
        self.draw.drop_bindings();
    }
}

/// A tile's draw's entries in the layer's storage buffers.
struct HillshadeUniforms {
    draw_ubo: HillshadeDrawableUBO,
    tile_props: HillshadeTilePropsUBO,
    projection: ProjectionUBO,
}

pub(crate) struct RenderHillshadeLayer {
    base: RenderLayerBase,
    values: LayerValues<Hillshade>,
    drawing: TileDraws<HillshadeTile>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<HillshadeUniforms>,
    maxzoom: u8,

    static_data_shared_vertices: Option<SharedVertices>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<SharedIndexes>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// Each DEM's render target, by the id of its tile's contents. They draw
    /// while the layer is renderable.
    dem_targets: HashMap<SimpleIdentity, DemTarget>,
    /// The quads of each tile's mask, by the id of its contents.
    mask_quads: MaskQuads,
    /// The contents whose targets the last frame prepared.
    prepared_in_frame: Vec<SimpleIdentity>,
    /// The contents of the frame's tiles, which keep their render targets
    /// and quads.
    live_dems: HashSet<SimpleIdentity>,
    /// The illumination of the evaluation the draws last drew with, before
    /// a viewport anchor turns it by the map's bearing.
    illumination: Option<(Arc<EvaluatedLayer<Hillshade>>, Illumination)>,
}

impl RenderHillshadeLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Hillshade>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderHillshadeLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            maxzoom: TERRAIN_RGB_MAXZOOM,
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            dem_targets: HashMap::default(),
            mask_quads: MaskQuads::default(),
            prepared_in_frame: Vec::new(),
            live_dems: HashSet::default(),
            illumination: None,
        }
    }
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

/// The latitudes of the north and the south edges of `id`, in degrees.
pub(crate) fn lat_range(id: &UnwrappedTileID) -> [f32; 2] {
    let latlng0 = LatLng::from_unwrapped_tile(id.canonical.z, id.canonical.x, id.canonical.y, id.wrap);
    let below = UnwrappedTileID::new(id.canonical.z, i64::from(id.canonical.x), i64::from(id.canonical.y) + 1);
    let latlng1 = LatLng::from_unwrapped_tile(below.canonical.z, below.canonical.x, below.canonical.y, below.wrap);
    [latlng0.latitude() as f32, latlng1.latitude() as f32]
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

/// Whether the light of the layer's values `evaluated` is anchored to the
/// viewport, so that it turns with the map's bearing.
fn lit_by_bearing(evaluated: &EvaluatedLayer<Hillshade>) -> bool {
    *evaluated.paint.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport
}

impl TypedLayer for RenderHillshadeLayer {
    type Type = Hillshade;
    type Drawing = TileDraws<HillshadeTile>;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

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

    /// Lit from the viewport, the surface shades its slopes with its lights
    /// at the frame's bearing, so that its drapes don't turn with the map.
    fn surface_light(&mut self, parameters: &PaintParameters<'_>) -> Option<SurfaceLight> {
        let evaluated = &self.values.evaluated;
        if !lit_by_bearing(evaluated) {
            return None;
        }
        let Illumination { props, num_lights } = Self::illumination(&mut self.illumination, evaluated, parameters);
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

    /// Makes the render target of each DEM it has none of, which prepares
    /// its slopes, and the frame's draws of the layer, those of each of its
    /// tiles, which shade the slopes and which it builds for a tile it keeps
    /// none of, and writes the layer's buffers they read. In a drape that
    /// the surface shades ([`PaintParameters::drape_lit`]), they draw the
    /// slopes as they were prepared, without lights.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        render_tiles: &[FrameTile<'_, Hillshade>],
    ) {
        let context = &mut *paint_parameters.context;
        let hillshade_prepare_shader =
            context.shader_key(&programs::HILLSHADE_PREPARE, Uniforms::default(), ProjectionVariant::Mercator);
        let hillshade_shader =
            context.shader_key(&programs::HILLSHADE, Uniforms::default(), self.base.projection_variant);
        let color_mode = ColorMode::alpha_blended();
        let static_data_shared_vertices = &*self
            .static_data_shared_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices()));
        let static_data_indices = &*self
            .static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())));

        // `resampling` filters the prepared texture the tile's draw samples;
        // the prepare pass reads the DEM with Nearest regardless.
        let resampling_filter = match *self.values.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };

        // The illumination, which the first draw computes.
        let mut illumination = None;
        for tile in render_tiles {
            let tile_id = tile.draw_id(paint_parameters);
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let Some(dem) = tile.bucket().filter(|dem| dem.has_data()) else {
                *entry.value = None;
                continue;
            };
            let dem_id = dem.id();
            // A draw built from other contents goes.
            entry.set_bucket(dem_id);

            let generation = dem.generation();
            let target = match self.dem_targets.entry(dem_id) {
                Entry::Occupied(entry) if entry.get().generation == generation => entry.into_mut(),
                entry => {
                    let attrs = hillshade_vertex_attributes(static_data_shared_vertices);
                    let target = DemTarget::new(
                        gpu,
                        paint_parameters.context,
                        dem,
                        &tile_id.to_unwrapped(),
                        self.maxzoom,
                        &hillshade_prepare_shader,
                        attrs,
                        static_data_indices,
                    );
                    entry.insert_entry(target).into_mut()
                }
            };
            let render_target_texture = target.render_target.texture();
            let context = &mut *paint_parameters.context;
            if let Some(mut texture) = context.texture_mut(render_target_texture) {
                texture.set_sampler(SamplerState {
                    filter: resampling_filter,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            let masking = &dem.masking;
            let geometry_generation = masking.generation();
            // The tile's geometry: the globe's mesh of its mask, the mask's
            // quads, or the whole tile's quad.
            let mut tile_geometry = || -> (SharedVertices, usize, DrawIndexes, Vec<SegmentBase>) {
                if self.base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        self.globe_meshes.get(context, &tile_id.canonical, masking.mask(), RasterLayoutVertex::new);
                    (
                        globe_mesh.vertices.clone(),
                        globe_mesh.vertices.count(),
                        globe_mesh.indexes(),
                        globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else if let Some(quads) = self.mask_quads.get(context, dem_id, masking) {
                    (
                        quads.vertices.clone(),
                        quads.vertices.count(),
                        quads.indexes.clone().into(),
                        quads.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else {
                    (
                        static_data_shared_vertices.clone(),
                        4,
                        static_data_indices.clone().into(),
                        RenderStaticData::raster_segments(),
                    )
                }
            };
            let mut draw_geometry = || {
                let (vertices, vertex_count, indexes, segments) = tile_geometry();
                DrawGeometry {
                    vertices: hillshade_vertex_attributes(&vertices),
                    vertex_count: if segments.is_empty() { 0 } else { vertex_count },
                    mode: DrawMode::Triangles(),
                    indexes,
                    segments,
                    instances: None,
                }
            };

            let cached = if let Some(cached) = entry.value {
                if cached.geometry_generation != geometry_generation {
                    cached.draw.objects.replace_geometry(draw_geometry());
                    cached.geometry_generation = geometry_generation;
                }
                // The DEM's render target is made again with its
                // generation.
                cached.draw.objects.set_texture(IMAGE_SLOT, Some(render_target_texture));
                cached
            } else {
                let mut textures = Textures::default();
                textures[IMAGE_SLOT] = Some(render_target_texture.clone());
                let objects = DrawObjects::new(
                    hillshade_shader.clone(),
                    color_mode,
                    CullFaceMode::disabled(),
                    draw_geometry(),
                    textures,
                );
                entry.value.insert(HillshadeTile { draw: KeptDraw::new(objects), geometry_generation })
            };
            let draw = cached.draw.frame_draw(Some(tile_id));
            let Illumination { num_lights, .. } = *illumination.get_or_insert_with(|| {
                Self::illumination(&mut self.illumination, &self.values.evaluated, paint_parameters)
            });
            let num_lights = if paint_parameters.drape_lit { 0 } else { num_lights };
            let uniforms =
                Self::uniforms(&self.values.evaluated.paint, &tile_id.to_unwrapped(), num_lights, paint_parameters);
            self.draw_list.push(draw, uniforms);
        }

        self.drawing.finish(&mut self.draw_list, gpu, paint_parameters.context, |writer| {
            if let Some(Illumination { props, .. }) = &illumination {
                writer.uniform(PROPS_ID, props);
            }
            writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
            writer.storage(TILE_PROPS_ID, |uniforms| uniforms.tile_props);
            writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
        });
    }

    /// Keeps the source's maximum zoom, which the DEMs' preparation reads,
    /// and drops the render targets and quads of the tiles the frame
    /// doesn't draw, in every frame, including one the layer draws nothing
    /// in.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, Hillshade>]) {
        if let Some(max_zoom) = params.source.and_then(RenderSource::max_zoom) {
            self.maxzoom = max_zoom;
        }
        let live_dems = &mut self.live_dems;
        live_dems.clear();
        live_dems.extend(tiles.iter().filter_map(|tile| Some(tile.bucket()?.id())));
        self.dem_targets.retain(|id, _| live_dems.contains(id));
        self.mask_quads.retain(live_dems);
    }

    fn release(&mut self) {
        self.dem_targets.clear();
        self.mask_quads.clear();
    }
}

impl RenderLayer for RenderHillshadeLayer {
    fn upload_render_targets(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.base.is_renderable {
            for target in self.dem_targets.values_mut().filter(|target| !target.prepared) {
                target.prepare.upload(gpu, context);
            }
        }
    }

    /// Prepares the slopes of the targets that don't hold them yet.
    fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.prepared_in_frame.clear();
        if !self.base.is_renderable {
            return;
        }
        for (&dem, target) in self.dem_targets.iter_mut().filter(|(_, target)| !target.prepared) {
            parameters.current_layer = 0;
            let Some(mut passes) = target.render_target.begin(gpu, parameters) else {
                continue;
            };
            let mut drawn = true;
            while passes.next(parameters) {
                drawn &= target.prepare.render(gpu, parameters);
            }
            // A target whose draw or pass failed prepares again in the next
            // frame.
            target.prepared = passes.end(gpu, parameters) && drawn;
            if target.prepared {
                self.prepared_in_frame.push(dem);
            }
        }
    }

    fn prepare_again(&mut self) {
        for dem in self.prepared_in_frame.drain(..) {
            if let Some(target) = self.dem_targets.get_mut(&dem) {
                target.prepared = false;
            }
        }
    }
}

impl DemTarget {
    /// The render target of the slopes of `dem`, which the draw of `shader`
    /// prepares from it, drawn over `attrs` with the `quad` of the
    /// static data, with the uniforms of `tile_id`, the tile it is made for,
    /// of a source of `maxzoom`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's GPU and context, and the tile's DEM, source, program and geometry"
    )]
    fn new(
        gpu: &mut dyn Gpu,
        context: &mut Context,
        dem: &DemContents,
        tile_id: &UnwrappedTileID,
        maxzoom: u8,
        shader: &ShaderKey,
        attrs: VertexAttributeArray,
        indexes: &SharedIndexes,
    ) -> DemTarget {
        let generation = dem.generation();
        let (tilesize, stride, encoding, dem_image) = {
            let dem = dem.dem();
            (dem.dim as u16, dem.stride, dem.encoding, dem.image().clone())
        };
        let render_target = RenderTarget::new(
            context,
            Size::new(u32::from(tilesize), u32::from(tilesize)),
            TextureChannelDataType::UnsignedByte,
        );

        // Nearest, as the Sobel kernel samples exact texel values.
        let dem = context.add_texture_with(|texture| {
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
        let objects =
            DrawObjects::new(shader.clone(), ColorMode::unblended(), CullFaceMode::disabled(), geometry, textures);
        let mut prepare = SingleDraw::new(objects, FrameDraw::new(None, SimpleIdentity::new()));
        let extent = f64::from(EXTENT);
        let m =
            orthographic(0.0, extent, -extent, 0.0, -1.0, 1.0) * DMat4::from_translation(DVec3::new(0.0, -extent, 0.0));
        bind_no_projection(gpu, &mut prepare.frame.uniform_buffers, context);
        let draw_ubo = HillshadePrepareDrawableUBO { matrix: cast_matrix(&m) };
        prepare.write_pod(gpu, PREPARE_DRAWABLE_ID, &draw_ubo, context);
        let unpack = DemUnpack::of(encoding);
        let tile_props = HillshadePrepareTilePropsUBO {
            unpack: unpack.weights_and_base(),
            dimension: [stride as f32, stride as f32],
            zoom: f32::from(tile_id.canonical.z),
            maxzoom: f32::from(maxzoom),
            unpack_scale: unpack.scale,
            ..HillshadePrepareTilePropsUBO::default()
        };
        prepare.write_pod(gpu, PREPARE_TILE_PROPS_ID, &tile_props, context);

        DemTarget { render_target, _dem: dem, prepare, generation, prepared: false }
    }
}

impl RenderHillshadeLayer {
    /// The entries in the layer's storage buffers of the draw of `tile_id`
    /// with the layer's values `evaluated`, lit by `num_lights` light
    /// sources.
    fn uniforms(
        evaluated: &HillshadePaintProperties::PossiblyEvaluated,
        tile_id: &UnwrappedTileID,
        num_lights: usize,
        parameters: &PaintParameters<'_>,
    ) -> HillshadeUniforms {
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
        HillshadeUniforms {
            draw_ubo: HillshadeDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) },
            tile_props: HillshadeTilePropsUBO {
                latrange: lat_range(tile_id),
                exaggeration: *evaluated.hillshade_exaggeration.uniform(),
                method: method_to_int(*evaluated.hillshade_method.uniform()),
                num_lights: num_lights as i32,
                pad0: 0.0,
                pad1: 0.0,
                pad2: 0.0,
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// The illumination of the layer's evaluation `layer` at the frame's
    /// bearing, which a viewport anchor turns its lights by. The
    /// evaluation's own is kept in `cache`, so that a turning map doesn't
    /// compute it again.
    fn illumination(
        cache: &mut Option<(Arc<EvaluatedLayer<Hillshade>>, Illumination)>,
        layer: &Arc<EvaluatedLayer<Hillshade>>,
        parameters: &PaintParameters<'_>,
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
            let bearing = parameters.view.angle() as f32;
            let lights = illumination.num_lights.min(illumination.props.azimuths.len());
            for azimuth in &mut illumination.props.azimuths[..lights] {
                *azimuth -= bearing;
            }
        }
        illumination
    }
}
