// Each DEM tile is first drawn into its own render target by the prepare
// shader (slopes from the DEM), then the tile's draw shades that texture.
// The layer keeps the render targets per bucket id
// (`bucket_render_targets`), with the bucket's prepare generation (see
// hillshade_bucket.rs), and a tile's draw takes the bucket's masked geometry
// again only when it changed.

use std::collections::hash_map::Entry;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::hillshade_layer_ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES,
};
use mln_gfx::shaders::hillshade_prepare_layer_ubo::{HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_HILLSHADE_DRAWABLE_UBO, ID_HILLSHADE_EVALUATED_PROPS_UBO, ID_HILLSHADE_IMAGE_TEXTURE,
    ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_PREPARE_DRAWABLE_UBO, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
    ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TILE_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, VertexAttributeArray};
use mln_math::deg2radf;
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Hillshade;
use mln_style::layers::hillshade_layer_properties::HillshadePaintProperties;
use mln_style_types::{HillshadeIlluminationAnchorType, HillshadeMethodType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::constants::{self, EXTENT};
use mln_util::geo::LatLng;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;
use mln_util::time::Instant;

use super::{cast_matrix, shared_vertices};
use crate::bucket::{Bucket, NoBucket};
use crate::buckets::hillshade_bucket::{HillshadeBucket, HillshadeLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::geometry::dem_data::get_unpack_vector;
use crate::gfx::context::Context;
use crate::gfx::index_vector::{IndexVector, IndexVectorPtr};
use crate::gfx::texture2d::OwnedTexture;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_drawing::{
    DrawDepth, DrawList, DrawObjectsMut, FrameDraw, KeptDraw, SingleDraw, TileDraws, TileObjects,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerParts, LayerSource, LayerTiles, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer,
    RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_target::RenderTarget;
use crate::tile_matrix::{bind_no_projection, get_projection_data, to_projection_ubo};

const HILLSHADE_PREPARE_SHADER_GROUP_NAME: &str = "HillshadePrepareShader";
const HILLSHADE_SHADER_GROUP_NAME: &str = "HillshadeShader";

/// The pass the layer's draws draw in, onto the frame and into its render
/// targets.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

impl LayerType for Hillshade {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<LayerStyle<Hillshade>>) -> Box<dyn RenderLayer> {
        Box::new(RenderHillshadeLayer::new(style))
    }
}

/// What the layer keeps of a bucket: the render target the slopes of its DEM
/// are prepared in, and the draw that prepares them.
struct BucketTarget {
    render_target: RenderTarget,
    /// The bucket's DEM, which `prepare` samples.
    _dem: OwnedTexture,
    /// The draw that prepares the slopes, with the uniforms it reads.
    prepare: SingleDraw,
    /// The bucket's prepare generation the target was made at.
    generation: u64,
}

/// What the layer keeps of a tile: the draw that shades the slopes of its
/// bucket, with when the bucket's geometry it took was made.
pub(crate) struct HillshadeTile {
    draw: KeptDraw,
    geometry_time: Instant,
}

impl TileObjects for HillshadeTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        self.draw.objects(())
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

    hillshade_prepare_shader: Option<ShaderProgramPtr>,
    hillshade_shader: Option<ShaderProgramPtr>,

    static_data_shared_vertices: Option<VertexVectorBasePtr>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<IndexVectorPtr>,
    globe_meshes: GlobeTileMeshCache<HillshadeLayoutVertex>,

    /// Each bucket's render target, by bucket id. They draw while the layer
    /// is renderable.
    bucket_render_targets: HashMap<SimpleIdentity, BucketTarget>,
    /// The buckets of the frame's tiles, which keep their render targets.
    live_buckets: HashSet<SimpleIdentity>,
    /// The illumination of the evaluation the draws last drew with, and its
    /// number of light sources: a viewport anchor's takes the bearing of the
    /// first frame that draws with it.
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
            maxzoom: constants::TERRAIN_RGB_MAXZOOM,
            hillshade_prepare_shader: None,
            hillshade_shader: None,
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            bucket_render_targets: HashMap::default(),
            live_buckets: HashSet::default(),
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

    // Pack shadow colors (4 colors * 4 components = 16 floats)
    for (i, color) in illumination.shadow_colors.iter().take(4).enumerate() {
        ubo.shadows[i * 4] = color.r;
        ubo.shadows[i * 4 + 1] = color.g;
        ubo.shadows[i * 4 + 2] = color.b;
        ubo.shadows[i * 4 + 3] = color.a;
    }

    // Pack highlight colors (4 colors * 4 components = 16 floats)
    for (i, color) in illumination.highlight_colors.iter().take(4).enumerate() {
        ubo.highlights[i * 4] = color.r;
        ubo.highlights[i * 4 + 1] = color.g;
        ubo.highlights[i * 4 + 2] = color.b;
        ubo.highlights[i * 4 + 3] = color.a;
    }

    ubo
}

/// Convert `HillshadeMethodType` enum to int for shader
fn method_to_int(method: HillshadeMethodType) -> i32 {
    method as i32
}

/// Calculate latitude range for tile
fn lat_range(id: &UnwrappedTileID) -> [f32; 2] {
    let latlng0 = LatLng::from_unwrapped_tile(id.canonical.z, id.canonical.x, id.canonical.y, id.wrap);
    let below = UnwrappedTileID::new(id.canonical.z, i64::from(id.canonical.x), i64::from(id.canonical.y) + 1);
    let latlng1 = LatLng::from_unwrapped_tile(below.canonical.z, below.canonical.x, below.canonical.y, below.wrap);
    [latlng0.latitude() as f32, latlng1.latitude() as f32]
}

/// The vertex attributes of a hillshade (or prepare) draw over
/// `vertices`.
fn hillshade_vertex_attributes(vertices: &VertexVectorBasePtr) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let stride = size_of::<HillshadeLayoutVertex>() as u32;
    if let Some(attr) = attrs.set(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(
            Some(Rc::clone(vertices)),
            0, // offsetof(HillshadeLayoutVertex, a1)
            0,
            stride,
            AttributeDataType::Short2,
        );
    }
    if let Some(attr) = attrs.set(ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(
            Some(Rc::clone(vertices)),
            4, // offsetof(HillshadeLayoutVertex, a2)
            0,
            stride,
            AttributeDataType::Short2,
        );
    }
    attrs
}

impl TypedLayer for RenderHillshadeLayer {
    type Type = Hillshade;
    type Drawing = TileDraws<HillshadeTile>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Drops the shader of the last projection, and the globe's meshes
    /// when the frame isn't on the globe.
    fn projection_changed(&mut self) {
        self.hillshade_shader = None;
        if self.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
    }

    fn passes(
        &mut self,
        _parameters: &PropertyEvaluationParameters,
        evaluated: &mut EvaluatedLayer<Hillshade>,
    ) -> RenderPass {
        if *evaluated.paint.hillshade_exaggeration.uniform() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        }
    }

    /// Makes the render target of each bucket it has none of, which prepares
    /// its slopes, and the frame's draws of the layer, those of each of its
    /// tiles, which shade the slopes and which it builds for a tile it keeps
    /// none of, and writes the layer's buffers they read.
    fn update_draws(
        &mut self,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        source: Option<LayerSource<'_, Hillshade>>,
    ) {
        let Some(render_tiles) = source.and_then(LayerSource::tiles) else {
            return;
        };
        let shaders = &paint_parameters.static_data.shaders;

        if self.hillshade_prepare_shader.is_none() {
            self.hillshade_prepare_shader =
                shaders.shader(HILLSHADE_PREPARE_SHADER_GROUP_NAME, ProjectionVariant::Mercator);
        }
        if self.hillshade_shader.is_none() {
            self.hillshade_shader = shaders.shader(HILLSHADE_SHADER_GROUP_NAME, self.base.projection_variant);
        }
        let (Some(hillshade_prepare_shader), Some(hillshade_shader)) =
            (self.hillshade_prepare_shader.clone(), self.hillshade_shader.clone())
        else {
            self.remove_all_draws();
            return;
        };
        if (RENDER_PASS.underlying_type() & self.base.evaluated_properties.render_passes()) == 0 {
            return;
        }

        let static_data_shared_vertices = Rc::clone(
            self.static_data_shared_vertices
                .get_or_insert_with(|| shared_vertices(&RenderStaticData::raster_vertices())),
        );
        let static_data_indices = Rc::clone(
            self.static_data_indices.get_or_insert_with(|| IndexVector::new(RenderStaticData::quad_triangle_indices())),
        );

        // `resampling` filters the prepared texture the tile's draw samples;
        // the prepare pass reads the DEM with Nearest regardless.
        let resampling_filter = match *self.values.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };

        // Drop the render targets of buckets that are no longer rendered.
        let live_buckets = &mut self.live_buckets;
        live_buckets.clear();
        live_buckets.extend(render_tiles.iter().filter_map(|tile| tile.dem().map(Bucket::get_id)));
        self.bucket_render_targets.retain(|id, _| live_buckets.contains(id));

        // The illumination, which the first draw computes.
        let mut illumination = None;
        for tile in render_tiles.iter() {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let Some(bucket) = tile.dem().filter(|bucket| bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket_id = bucket.base.bucket_id;
            // A draw built from another bucket goes.
            entry.set_bucket(bucket_id);

            let generation = bucket.render_target_generation();
            let render_target_texture = match self.bucket_render_targets.entry(bucket_id) {
                Entry::Occupied(entry) if entry.get().generation == generation => entry.get().render_target.texture(),
                entry => {
                    let attrs = hillshade_vertex_attributes(&static_data_shared_vertices);
                    let target = BucketTarget::new(
                        paint_parameters.context,
                        bucket,
                        &tile_id.to_unwrapped(),
                        self.maxzoom,
                        &hillshade_prepare_shader,
                        attrs,
                        &static_data_indices,
                    );
                    let texture = target.render_target.texture();
                    entry.insert_entry(target);
                    texture
                }
            };
            if let Some(mut texture) = paint_parameters.context.texture_mut(render_target_texture) {
                texture.set_sampler(SamplerState {
                    filter: resampling_filter,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            let geometry = bucket.geometry();
            let geometry_time = geometry.last_modified;
            // The tile's geometry: the globe's mesh of its mask, the mask's
            // geometry, or the whole tile's quad.
            let mut tile_geometry = || -> (VertexVectorBasePtr, usize, IndexVectorPtr, Vec<SegmentBase>) {
                if self.base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        self.globe_meshes.get(&tile_id.canonical, &geometry.mask, HillshadeBucket::layout_vertex);
                    let vertices: VertexVectorBasePtr =
                        Rc::<mln_gfx::vertex_vector::VertexVector<HillshadeLayoutVertex>>::clone(&globe_mesh.vertices);
                    (
                        vertices,
                        globe_mesh.vertices.elements(),
                        Rc::clone(&globe_mesh.indices),
                        globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else if !geometry.vertices.is_empty() && !geometry.indices.is_empty() && !geometry.segments.is_empty()
                {
                    (
                        shared_vertices(&geometry.vertices),
                        geometry.vertices.len(),
                        IndexVector::new(geometry.indices.clone()),
                        geometry.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else {
                    (
                        Rc::clone(&static_data_shared_vertices),
                        4,
                        Rc::clone(&static_data_indices),
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
                if cached.geometry_time != geometry_time {
                    cached.draw.objects.replace_geometry(draw_geometry());
                    cached.geometry_time = geometry_time;
                }
                // The bucket's render target is made again with its
                // generation.
                cached.draw.objects.set_texture(ID_HILLSHADE_IMAGE_TEXTURE, Some(render_target_texture));
                cached
            } else {
                let mut textures = Textures::default();
                textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(render_target_texture);
                let objects = DrawObjects::new(
                    Rc::clone(&hillshade_shader),
                    ColorMode::alpha_blended(),
                    CullFaceMode::disabled(),
                    draw_geometry(),
                    textures,
                );
                entry.value.insert(HillshadeTile { draw: KeptDraw::new(objects), geometry_time })
            };
            let draw = FrameDraw { depth: DrawDepth::Off, ..cached.draw.frame_draw(Some(tile_id)) };
            let Illumination { num_lights, .. } = *illumination.get_or_insert_with(|| {
                Self::illumination(&mut self.illumination, &self.values.evaluated, paint_parameters)
            });
            let uniforms =
                Self::uniforms(&self.values.evaluated.paint, &tile_id.to_unwrapped(), num_lights, paint_parameters);
            self.draw_list.push(draw, uniforms);
        }

        let frame = self.drawing.frame_mut();
        self.draw_list.finish(frame, paint_parameters.context, |writer| {
            if let Some(Illumination { props, .. }) = &illumination {
                writer.uniform(ID_HILLSHADE_EVALUATED_PROPS_UBO, props);
            }
            writer.storage(ID_HILLSHADE_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
            writer.storage(ID_HILLSHADE_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
            writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
        });
    }

    /// Keeps the source's maximum zoom, which the DEMs' preparation reads.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, _tiles: Option<LayerTiles<'_, Hillshade>>) {
        if let Some(source) = params.source {
            self.maxzoom = source.max_zoom();
        }
    }

    fn release(&mut self) {
        self.bucket_render_targets.clear();
    }
}

impl RenderLayer for RenderHillshadeLayer {
    fn upload_render_targets(&mut self, context: &mut Context) {
        if self.base.is_renderable {
            for target in self.bucket_render_targets.values_mut() {
                target.prepare.upload(context);
            }
        }
    }

    fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if !self.base.is_renderable {
            return;
        }
        for BucketTarget { render_target, prepare, .. } in self.bucket_render_targets.values_mut() {
            parameters.current_layer = 0;
            let Some(mut passes) = render_target.begin(gpu, parameters) else {
                continue;
            };
            while passes.next(parameters) {
                prepare.render(gpu, parameters);
            }
            passes.end(gpu, parameters);
        }
    }
}

impl BucketTarget {
    /// The render target of `bucket`'s slopes, which the draw of `shader`
    /// prepares from its DEM, drawn over `attrs` with the `quad` of the
    /// static data, with the uniforms of `tile_id`, the tile it is made for,
    /// of a source of `maxzoom`.
    fn new(
        context: &mut Context,
        bucket: &HillshadeBucket,
        tile_id: &UnwrappedTileID,
        maxzoom: u8,
        shader: &ShaderProgramPtr,
        attrs: VertexAttributeArray,
        indexes: &IndexVectorPtr,
    ) -> BucketTarget {
        let (tilesize, stride, encoding, dem_image) = {
            let dem = bucket.get_dem_data();
            (dem.dim as u16, dem.stride, dem.encoding, dem.get_image().clone())
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
            indexes: Rc::clone(indexes),
            segments: RenderStaticData::raster_segments(),
            instances: None,
        };
        let mut textures = Textures::default();
        textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(dem.key());
        let objects =
            DrawObjects::new(Rc::clone(shader), ColorMode::unblended(), CullFaceMode::disabled(), geometry, textures);
        let mut prepare = SingleDraw::new(objects, FrameDraw::new(None, SimpleIdentity::new()));
        let mut m: Mat4 = [0.0; 16];
        let extent = f64::from(EXTENT);
        matrix::ortho(&mut m, 0.0, extent, -extent, 0.0, -1.0, 1.0);
        let a = m;
        matrix::translate(&mut m, &a, 0.0, -extent, 0.0);
        let buffers = &mut prepare.frame.uniform_buffers;
        bind_no_projection(buffers, context);
        let draw_ubo = HillshadePrepareDrawableUBO { matrix: cast_matrix(&m) };
        buffers.create_or_update_pod(ID_HILLSHADE_PREPARE_DRAWABLE_UBO, &draw_ubo, context);
        let tile_props = HillshadePrepareTilePropsUBO {
            unpack: get_unpack_vector(encoding),
            dimension: [stride as f32, stride as f32],
            zoom: f32::from(tile_id.canonical.z),
            maxzoom: f32::from(maxzoom),
        };
        buffers.create_or_update_pod(ID_HILLSHADE_PREPARE_TILE_PROPS_UBO, &tile_props, context);

        BucketTarget { render_target, _dem: dem, prepare, generation: bucket.render_target_generation() }
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
        let projection = get_projection_data(
            tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            true,
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

    /// The illumination of the layer's evaluation `layer`, kept in `cache`,
    /// which this computes with the frame's bearing the first time it draws
    /// with the evaluation.
    fn illumination(
        cache: &mut Option<(Arc<EvaluatedLayer<Hillshade>>, Illumination)>,
        layer: &Arc<EvaluatedLayer<Hillshade>>,
        parameters: &PaintParameters<'_>,
    ) -> Illumination {
        if let Some((cached_layer, illumination)) = cache
            && Arc::ptr_eq(cached_layer, layer)
        {
            return *illumination;
        }
        let evaluated = &layer.paint;
        let mut illumination = illumination_properties(evaluated);
        if *evaluated.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport {
            let bearing = parameters.state.get_bearing() as f32;
            for azimuth in &mut illumination.direction_radians {
                *azimuth -= bearing;
            }
        }
        let props = pack_evaluated_props(&illumination, evaluated.hillshade_accent_color.uniform());
        let illumination = Illumination { props, num_lights: illumination.direction_radians.len() };
        *cache = Some((Arc::clone(layer), illumination));
        illumination
    }
}
