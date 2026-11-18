// Each DEM tile is first drawn into its own render target by the prepare
// shader (slopes from the DEM), then the tile's draw shades that texture.
//
// Differences in mechanics, not in output:
// - C++ keeps the prepare render target in the (shared) HillshadeBucket;
//   here the layer keeps it per bucket id (`bucket_render_targets`), with
//   the bucket's prepare generation (see hillshade_bucket.rs).
// - C++ re-applies the vertex attributes to existing tile drawables every
//   frame; the port does so only when the bucket's masked geometry changed
//   since the tile's draw took it.

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
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
use crate::draw_cache::{DrawCaches, DrawDepth, DrawSlots, Draws, FrameDraw, FrameDraws, KeptDraw, LayerDrawing};
use crate::geometry::dem_data::get_unpack_vector;
use crate::gfx::context::Context;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerParts, LayerPrepareParameters, LayerSource, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase,
    TypedLayer, set_layer_tiles,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_target::RenderTarget;
use crate::tile_matrix::{bind_no_projection, get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;

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
    /// The draw that prepares the slopes, with the uniforms it reads.
    prepare: Draws<(), KeptDraw>,
    /// The bucket's prepare generation the target was made at.
    generation: u64,
}

/// What the layer keeps of a tile: the draw that shades the slopes of its
/// bucket, with when the bucket's geometry it took was made.
pub(crate) struct HillshadeTile {
    draw: KeptDraw,
    geometry_time: Instant,
}

impl DrawSlots for HillshadeTile {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.draw.slot_mut(slot)
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
    drawing: LayerDrawing<HillshadeTile>,
    maxzoom: u8,

    hillshade_prepare_shader: Option<ShaderProgramPtr>,
    hillshade_shader: Option<ShaderProgramPtr>,

    static_data_shared_vertices: Option<VertexVectorBasePtr>,
    globe_meshes: GlobeTileMeshCache<HillshadeLayoutVertex>,

    /// Each bucket's render target, by bucket id. They draw while the layer
    /// is renderable.
    bucket_render_targets: HashMap<SimpleIdentity, BucketTarget>,
    /// The illumination of the evaluation the draws last drew with: a
    /// viewport anchor's takes the bearing of the first frame that draws
    /// with it.
    evaluated_props: Option<(Arc<EvaluatedLayer<Hillshade>>, HillshadeEvaluatedPropsUBO)>,
}

impl RenderHillshadeLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Hillshade>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderHillshadeLayer {
            base,
            values,
            drawing: LayerDrawing::default(),
            maxzoom: constants::TERRAIN_RGB_MAXZOOM,
            hillshade_prepare_shader: None,
            hillshade_shader: None,
            static_data_shared_vertices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            bucket_render_targets: HashMap::default(),
            evaluated_props: None,
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

impl IlluminationProperties {
    fn num_sources(&self) -> usize {
        self.direction_radians.len()
    }
}

/// Pad shorter arrays by repeating the last element.
fn pad_array<T: Clone + Default>(arr: &mut Vec<T>, max_length: usize) {
    let last = arr.last().cloned().unwrap_or_default();
    let length = max_length.max(arr.len()).max(1);
    arr.resize(length, last);
}

/// Convert style properties to illumination properties
fn get_illumination_properties(evaluated: &HillshadePaintProperties::PossiblyEvaluated) -> IlluminationProperties {
    // Get the values from evaluated properties (these are now vectors)
    let mut directions: Vec<f32> = evaluated.hillshade_illumination_direction.clone().uniform().clone();
    let mut altitudes: Vec<f32> = evaluated.hillshade_illumination_altitude.clone().uniform().clone();
    let mut highlights: Vec<Color> = evaluated.hillshade_highlight_color.clone().uniform().clone();
    let mut shadows: Vec<Color> = evaluated.hillshade_shadow_color.clone().uniform().clone();

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
fn get_lat_range(id: &UnwrappedTileID) -> [f32; 2] {
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
    type Tile = HillshadeTile;
    type Shared = ();

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }
}

impl RenderLayer for RenderHillshadeLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let properties = self.values.evaluate(parameters);

        self.base.passes = if *properties.paint.hillshade_exaggeration.uniform() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        };
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn prepare(&mut self, params: &LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        let order = params.source.and_then(|source| {
            self.maxzoom = source.get_max_zoom();
            source.tile_order()
        });
        set_layer_tiles(self, params, order)
    }

    fn release_render_data(&mut self) {
        crate::render_layer::release_render_data_base(self);
        self.bucket_render_targets.clear();
    }

    fn upload_render_targets(&mut self, context: &Context) {
        if self.base.is_renderable {
            for target in self.bucket_render_targets.values_mut() {
                target.prepare.upload(context);
            }
        }
    }

    fn render_targets(&mut self, parameters: &mut PaintParameters<'_>) {
        if !self.base.is_renderable {
            return;
        }
        for BucketTarget { render_target, prepare, .. } in self.bucket_render_targets.values_mut() {
            parameters.current_layer = 0;
            render_target.render(parameters, |gpu, parameters| prepare.render(gpu, parameters));
        }
    }

    /// Makes the render target of each bucket it has none of, which prepares
    /// its slopes, and the frame's draws of the layer, those of each of its
    /// tiles, which shade the slopes and which it builds for a tile it keeps
    /// none of, and writes the layer's buffers they read.
    fn update(
        &mut self,
        paint_parameters: &PaintParameters<'_>,
        _update_parameters: &UpdateParameters,
        source: Option<LayerSource<'_>>,
    ) {
        let tiles = source.and_then(LayerSource::tiles);
        let (shaders, context, state) =
            (&paint_parameters.static_data.shaders, paint_parameters.context, paint_parameters.state);
        if self.update_projection_variant(state) {
            self.hillshade_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let Some(render_tiles) = tiles.filter(|tiles| !tiles.is_empty()) else {
            self.remove_all_draws();
            return;
        };
        self.drawing.frame.get_or_insert_with(FrameDraws::new);

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
        let static_data_indices: IndexVectorBasePtr =
            Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices())));
        let static_data_segments = RenderStaticData::raster_segments();

        // `resampling` filters the prepared texture the tile's draw samples;
        // the prepare pass reads the DEM with Nearest regardless.
        let resampling_filter = match *self.values.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };

        // Drop the render targets of buckets that are no longer rendered
        // (C++ releases them with the bucket).
        let live_buckets: HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.style).map(Bucket::get_id)).collect();
        self.bucket_render_targets.retain(|id, _| live_buckets.contains(id));

        let mut frame_draws: Vec<(FrameDraw, HillshadeUniforms)> = Vec::new();
        for tile in render_tiles.iter() {
            let tile_id = *tile.tile.get_overscaled_tile_id();

            let bucket = tile.get_bucket(&*self.base.style).filter(|b| b.has_data());
            let Some(bucket) = bucket.and_then(|bucket| bucket.downcast_ref::<HillshadeBucket>()) else {
                self.drawing.caches.drop_cache(&tile_id);
                continue;
            };
            let bucket_id = bucket.base.bucket_id;

            if self.drawing.caches.bucket(&tile_id).is_some_and(|prev| prev != bucket_id) {
                // The tile's draw was built from another bucket.
                self.drawing.caches.drop_cache(&tile_id);
            }
            self.drawing.caches.set_bucket(&tile_id, bucket_id);

            let generation = bucket.render_target_generation();
            let render_target_texture = match self.bucket_render_targets.entry(bucket_id) {
                Entry::Occupied(entry) if entry.get().generation == generation => {
                    entry.get().render_target.get_texture()
                }
                entry => {
                    let attrs = hillshade_vertex_attributes(&static_data_shared_vertices);
                    let target = BucketTarget::new(
                        context,
                        bucket,
                        &tile_id.to_unwrapped(),
                        self.maxzoom,
                        &hillshade_prepare_shader,
                        attrs,
                        (&static_data_indices, &static_data_segments),
                    );
                    let texture = target.render_target.get_texture();
                    entry.insert_entry(target);
                    texture
                }
            };
            render_target_texture.borrow_mut().set_sampler(SamplerState {
                filter: resampling_filter,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });

            let geometry = bucket.geometry();
            let geometry_time = geometry.last_modified;
            // The tile's geometry: the globe's mesh of its mask, the mask's
            // geometry, or the whole tile's quad.
            let mut tile_geometry = || -> (VertexVectorBasePtr, usize, IndexVectorBasePtr, Vec<SegmentBase>) {
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
                        Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone()))),
                        geometry.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else {
                    (
                        Rc::clone(&static_data_shared_vertices),
                        4,
                        Rc::clone(&static_data_indices),
                        static_data_segments.iter().map(SegmentBase::copy).collect(),
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

            if let Some(cached) = self.drawing.caches.cache_mut(&tile_id) {
                if cached.geometry_time != geometry_time {
                    cached.draw.objects.replace_geometry(draw_geometry());
                    cached.geometry_time = geometry_time;
                }
                // The bucket's render target is made again with its
                // generation.
                cached.draw.objects.textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(render_target_texture);
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
                self.drawing.caches.set_cache(&tile_id, HillshadeTile { draw: KeptDraw::new(objects), geometry_time });
            }
            let Some(cached) = self.drawing.caches.cache(&tile_id) else {
                continue;
            };
            let draw = FrameDraw { depth: DrawDepth::Off, ..cached.draw.frame_draw(Some(tile_id)) };
            frame_draws.push((draw, self.uniforms(&tile_id.to_unwrapped(), paint_parameters)));
        }

        frame_draws.sort_by_key(|(draw, _)| draw.order());
        let props = (!frame_draws.is_empty()).then(|| self.evaluated_props(paint_parameters));
        let frame = self.drawing.frame.get_or_insert_with(FrameDraws::new);
        if let Some(props) = props {
            let buffers = &mut frame.uniform_buffers;
            buffers.create_or_update_pod(ID_HILLSHADE_EVALUATED_PROPS_UBO, &props, context);
            let draw_ubos: Vec<HillshadeDrawableUBO> =
                frame_draws.iter().map(|(_, uniforms)| uniforms.draw_ubo).collect();
            buffers.write_storage(ID_HILLSHADE_DRAWABLE_UBO, bytemuck::cast_slice(&draw_ubos), context);
            let tile_props: Vec<HillshadeTilePropsUBO> =
                frame_draws.iter().map(|(_, uniforms)| uniforms.tile_props).collect();
            buffers.write_storage(ID_HILLSHADE_TILE_PROPS_UBO, bytemuck::cast_slice(&tile_props), context);
            let projections: Vec<ProjectionUBO> = frame_draws.iter().map(|(_, uniforms)| uniforms.projection).collect();
            buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projections), context);
        }
        frame.draws.clear();
        frame.draws.extend(frame_draws.into_iter().map(|(draw, _)| draw));
    }
}

impl BucketTarget {
    /// The render target of `bucket`'s slopes, which the draw of `shader`
    /// prepares from its DEM, drawn over `attrs` with the `quad` of the
    /// static data, with the uniforms of `tile_id`, the tile it is made for,
    /// of a source of `maxzoom`.
    fn new(
        context: &Context,
        bucket: &HillshadeBucket,
        tile_id: &UnwrappedTileID,
        maxzoom: u8,
        shader: &ShaderProgramPtr,
        attrs: VertexAttributeArray,
        quad: (&IndexVectorBasePtr, &[SegmentBase]),
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

        let texture = context.create_texture_2d();
        {
            let mut t = texture.borrow_mut();
            t.set_image(Some(Rc::new(dem_image)));
            // Use Nearest filtering to match GL JS behavior - the Sobel
            // kernel samples exact pixel values
            t.set_sampler(SamplerState {
                filter: TextureFilterType::Nearest,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }
        let (indexes, segments) = quad;
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: if segments.is_empty() { 0 } else { 4 },
            mode: DrawMode::Triangles(),
            indexes: Rc::clone(indexes),
            segments: segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        let mut textures = Textures::default();
        textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(texture);
        let objects =
            DrawObjects::new(Rc::clone(shader), ColorMode::unblended(), CullFaceMode::disabled(), geometry, textures);
        let draw = KeptDraw::new(objects);
        let frame_draw = draw.frame_draw(None);
        let mut prepare = Draws::new(DrawCaches::of_layer(draw));
        prepare.frame.draws.push(frame_draw);
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

        BucketTarget { render_target, prepare, generation: bucket.render_target_generation() }
    }
}

impl RenderHillshadeLayer {
    /// The entries in the layer's storage buffers of the draw of `tile_id`.
    fn uniforms(&self, tile_id: &UnwrappedTileID, parameters: &PaintParameters<'_>) -> HillshadeUniforms {
        let evaluated = &self.values.evaluated.paint;
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
                latrange: get_lat_range(tile_id),
                exaggeration: *evaluated.hillshade_exaggeration.uniform(),
                method: method_to_int(*evaluated.hillshade_method.uniform()),
                num_lights: get_illumination_properties(evaluated).num_sources() as i32,
                pad0: 0.0,
                pad1: 0.0,
                pad2: 0.0,
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// The illumination of the layer's evaluation, which it computes with
    /// the frame's bearing the first time it draws with the evaluation.
    fn evaluated_props(&mut self, parameters: &PaintParameters<'_>) -> HillshadeEvaluatedPropsUBO {
        if let Some((layer, props)) = &self.evaluated_props
            && Arc::ptr_eq(layer, &self.values.evaluated)
        {
            return *props;
        }
        let evaluated = &self.values.evaluated.paint;
        let mut illumination = get_illumination_properties(evaluated);
        if *evaluated.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport {
            let bearing = parameters.state.get_bearing() as f32;
            for azimuth in &mut illumination.direction_radians {
                *azimuth -= bearing;
            }
        }
        let props = pack_evaluated_props(&illumination, evaluated.hillshade_accent_color.uniform());
        self.evaluated_props = Some((Arc::clone(&self.values.evaluated), props));
        props
    }
}
