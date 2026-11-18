// Differences in mechanics, not in output:
// - C++ re-applies the vertex attributes and creates new DEM textures for
//   existing tile drawables every frame; the port builds a tile's draw again
//   only when the bucket's masked geometry changed, and keeps the DEM
//   texture per bucket id.
// - The elevation stops texture is created anew when the ramp changes (C++
//   re-uploads into the same texture; its size may change with the ramp).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::TextureFormat;
use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::color_relief_layer_ubo::{
    ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_DRAWABLE_UBO, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
    ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, ID_COLOR_RELIEF_IMAGE_TEXTURE, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE,
    ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TILE_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr, VertexAttributeArray};
use mln_style::color_ramp_property_value::ColorRampPropertyValue;
use mln_style::expression::{
    Dependency, EvaluationContext, GlobalStateMap, from_expression_value, global_state_refs_intersect,
};
use mln_style::layer_type::LayerStyle;
use mln_style::layers::ColorRelief;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_style_types::ResamplingType;
use mln_util::Color;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::time::Instant;

use super::{cast_matrix, shared_vertices};
use crate::bucket::{Bucket, NoBucket};
use crate::buckets::hillshade_bucket::{HillshadeBucket, HillshadeLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::draw_cache::{DrawDepth, DrawSlots, FrameDraw, FrameDraws, KeptDraw, LayerDrawing};
use crate::gfx::texture2d::Texture2DPtr;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_type::LayerType;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    LayerParts, LayerPrepareParameters, LayerSource, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase,
    TypedLayer, set_layer_tiles,
};
use crate::render_pass::RenderPass;
use crate::render_source::RenderSource;
use crate::render_static_data::RenderStaticData;
use crate::tile_matrix::to_projection_ubo;
use crate::update_parameters::UpdateParameters;

const COLOR_RELIEF_SHADER_GROUP_NAME: &str = "ColorReliefShader";

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// What the layer keeps of a tile: the draw that colors its bucket's DEM,
/// with when it was built and the DEM's properties it was built with.
pub(crate) struct ReliefTile {
    draw: KeptDraw,
    built: Instant,
    tile_props: ColorReliefTilePropsUBO,
}

impl DrawSlots for ReliefTile {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.draw.slot_mut(slot)
    }
}

/// A draw's entries in the layer's storage buffers.
struct ReliefUniforms {
    draw_ubo: ColorReliefDrawableUBO,
    tile_props: ColorReliefTilePropsUBO,
    projection: ProjectionUBO,
}

impl LayerType for ColorRelief {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<LayerStyle<ColorRelief>>) -> Box<dyn RenderLayer> {
        Box::new(RenderColorReliefLayer::new(style))
    }
}

pub(crate) struct RenderColorReliefLayer {
    base: RenderLayerBase,
    values: LayerValues<ColorRelief>,
    drawing: LayerDrawing<ReliefTile>,

    // Color ramp data
    color_ramp_size: u32,
    color_ramp_changed: bool,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,

    /// Elevation values for each stop (RGBA floats, elevation in R).
    elevation_stops_data: Vec<f32>,
    /// RGB colors for each stop
    color_stops: PremultipliedImage,

    // GPU textures
    elevation_stops_texture: Option<Texture2DPtr>,
    color_stops_texture: Option<Texture2DPtr>,

    color_relief_shader: Option<ShaderProgramPtr>,

    static_data_shared_vertices: Option<VertexVectorBasePtr>,
    globe_meshes: GlobeTileMeshCache<HillshadeLayoutVertex>,

    /// The DEM texture of each bucket, with the bucket's prepare generation
    /// it was made at (C++ makes one per drawable and frame from the
    /// current, possibly backfilled, DEM image).
    dem_textures: HashMap<SimpleIdentity, (Texture2DPtr, u64)>,
}

impl RenderColorReliefLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<ColorRelief>>) -> Self {
        let (base, values) = LayerValues::new(impl_);

        // Initialize color ramp data with RGBA format (4 floats per stop)
        let color_ramp_size = 256;
        let mut layer = RenderColorReliefLayer {
            base,
            values,
            drawing: LayerDrawing::default(),
            color_ramp_size,
            color_ramp_changed: true,
            color_ramp_global_state: None,
            elevation_stops_data: vec![0.0; color_ramp_size as usize * 4],
            color_stops: PremultipliedImage::new(Size::new(color_ramp_size, 1)),
            elevation_stops_texture: None,
            color_stops_texture: None,
            color_relief_shader: None,
            static_data_shared_vertices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            dem_textures: HashMap::default(),
        };

        // Initialize with default color ramp immediately to avoid
        // uninitialized state
        layer.update_color_ramp();
        layer
    }

    fn update_color_ramp(&mut self) {
        // Get the color property value, using default if undefined
        let mut color_value = self.values.unevaluated.color_relief_color.get_value().clone();
        if color_value.is_undefined() {
            color_value = ColorRampPropertyValue::default();
            if color_value.is_undefined() {
                // If even the default is undefined, we can't proceed
                return;
            }
        }

        let mut elevation_stops_vector: Vec<f32> = Vec::new();
        let mut color_stops_vector: Vec<Color> = Vec::new();

        let expr = color_value.get_expression();

        if let Some(interpolate) = expr.as_interpolate() {
            let stop_count = interpolate.get_stop_count();

            elevation_stops_vector.reserve(stop_count);
            color_stops_vector.reserve(stop_count);

            // Extract elevation values from stops
            interpolate.each_stop(|elevation, _output_expr| {
                elevation_stops_vector.push(elevation as f32);
            });

            // Evaluate expression at each elevation to get colors
            for &elevation in &elevation_stops_vector {
                // Create evaluation context with elevation as color ramp
                // parameter
                let context = EvaluationContext::with_color_ramp(None, None, Some(f64::from(elevation)))
                    .with_global_state(self.color_ramp_global_state.as_deref());

                // Default to transparent black
                let color = match expr.evaluate(&context) {
                    Ok(result) => from_expression_value::<Color>(&result).unwrap_or(Color::new(0.0, 0.0, 0.0, 0.0)),
                    Err(_) => Color::new(0.0, 0.0, 0.0, 0.0),
                };

                color_stops_vector.push(color);
            }
        } else {
            // Fallback: Sample the color ramp uniformly
            const NUM_SAMPLES: u32 = 256;
            const MIN_ELEVATION: f32 = -500.0;
            const MAX_ELEVATION: f32 = 9000.0;

            elevation_stops_vector.reserve(NUM_SAMPLES as usize);
            color_stops_vector.reserve(NUM_SAMPLES as usize);

            for i in 0..NUM_SAMPLES {
                let t = i as f32 / (NUM_SAMPLES - 1) as f32;
                let elevation = MIN_ELEVATION + t * (MAX_ELEVATION - MIN_ELEVATION);
                elevation_stops_vector.push(elevation);

                let color = color_value.evaluate_ramp(f64::from(elevation), self.color_ramp_global_state.as_deref());
                color_stops_vector.push(color);
            }
        }

        let ramp_size = elevation_stops_vector.len() as u32;
        if ramp_size == 0 {
            return;
        }

        // Resize and prepare structures
        self.elevation_stops_data.resize(ramp_size as usize * 4, 0.0); // RGBA float for compatibility
        // `colorStops->resize({rampSize, 1})`
        let mut color_stops = PremultipliedImage::new(Size::new(ramp_size, 1));
        let keep = color_stops.data.len().min(self.color_stops.data.len());
        color_stops.data[..keep].copy_from_slice(&self.color_stops.data[..keep]);
        self.color_ramp_size = ramp_size;

        for i in 0..ramp_size as usize {
            // Store elevation in the R channel of an RGBA float vector
            self.elevation_stops_data[i * 4] = elevation_stops_vector[i]; // R = elevation
            self.elevation_stops_data[i * 4 + 1] = 0.0; // G = unused
            self.elevation_stops_data[i * 4 + 2] = 0.0; // B = unused
            self.elevation_stops_data[i * 4 + 3] = 1.0; // A = unused

            // Store colors without premultiplication for proper
            // interpolation
            let color = color_stops_vector[i];
            color_stops.data[i * 4] = (color.r * 255.0) as u8;
            color_stops.data[i * 4 + 1] = (color.g * 255.0) as u8;
            color_stops.data[i * 4 + 2] = (color.b * 255.0) as u8;
            color_stops.data[i * 4 + 3] = (color.a * 255.0) as u8;
        }
        self.color_stops = color_stops;

        self.color_ramp_changed = true;
    }
}

/// The vertex attributes of a color-relief draw over `vertices`.
fn color_relief_vertex_attributes(vertices: &VertexVectorBasePtr) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let stride = size_of::<HillshadeLayoutVertex>() as u32;
    if let Some(attr) = attrs.set(ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(
            Some(Rc::clone(vertices)),
            0, // offsetof(HillshadeLayoutVertex, a1)
            0,
            stride,
            AttributeDataType::Short2,
        );
    }
    if let Some(attr) = attrs.set(ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
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

impl TypedLayer for RenderColorReliefLayer {
    type Type = ColorRelief;
    type Tile = ReliefTile;
    type Shared = ();

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }
}

impl RenderLayer for RenderColorReliefLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = self.values.unevaluated.color_relief_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp();
        }

        let properties = self.values.evaluate(parameters);

        self.base.passes = if *properties.paint.color_relief_opacity.uniform() > 0.0 {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn release_render_data(&mut self) {
        crate::render_layer::release_render_data_base(self);
        self.dem_textures.clear();
    }

    fn prepare(&mut self, params: &LayerPrepareParameters<'_>) -> Option<Vec<usize>> {
        let order = params.source.and_then(RenderSource::tile_order);
        set_layer_tiles(self, params, order)
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
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
            self.color_relief_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let Some(render_tiles) = tiles.filter(|tiles| !tiles.is_empty()) else {
            self.remove_all_draws();
            return;
        };
        self.drawing.frame.get_or_insert_with(FrameDraws::new);

        if self.color_relief_shader.is_none() {
            self.color_relief_shader = shaders.shader(COLOR_RELIEF_SHADER_GROUP_NAME, self.base.projection_variant);
        }
        let Some(color_relief_shader) = self.color_relief_shader.clone() else {
            Log::error(Event::Render, "ColorRelief shader failed to load");
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

        // Update color ramp textures if changed
        if self.color_ramp_changed {
            let ramp_changed_texture = context.create_texture_2d();
            {
                let mut t = ramp_changed_texture.borrow_mut();
                // Use RGBA32F instead of R32F for llvmpipe compatibility
                t.set_format(TextureFormat::Rgba32Float);
                let data = bytemuck::cast_slice(&self.elevation_stops_data);
                if let Err(error) = t.upload_data(context, data, Size::new(self.color_ramp_size, 1)) {
                    Log::error(Event::Render, format!("Uploading the elevation stops failed: {error}"));
                }
                t.set_sampler(SamplerState {
                    filter: TextureFilterType::Nearest,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }
            self.elevation_stops_texture = Some(ramp_changed_texture);

            let color_stops_texture =
                Rc::clone(self.color_stops_texture.get_or_insert_with(|| context.create_texture_2d()));
            {
                let mut t = color_stops_texture.borrow_mut();
                t.set_image(Some(Rc::new(self.color_stops.clone())));
                t.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            self.color_ramp_changed = false;
        }

        // Skip rendering if color ramp textures aren't ready
        let (Some(elevation_stops_texture), Some(color_stops_texture)) =
            (self.elevation_stops_texture.clone(), self.color_stops_texture.clone())
        else {
            return;
        };

        // Drop the DEM textures of buckets that are no longer rendered.
        let live_buckets: HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.style).map(Bucket::get_id)).collect();
        self.dem_textures.retain(|id, _| live_buckets.contains(id));

        let dem_sampler = SamplerState {
            filter: match *self.values.evaluated.paint.resampling.uniform() {
                ResamplingType::Linear => TextureFilterType::Linear,
                ResamplingType::Nearest => TextureFilterType::Nearest,
            },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        };

        let mut frame_draws: Vec<(FrameDraw, ReliefUniforms)> = Vec::new();
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

            let geometry = bucket.geometry();
            let geometry_time = geometry.last_modified;
            let dem_data = bucket.get_dem_data();
            if dem_data.get_image().valid() {
                let tile_props = ColorReliefTilePropsUBO {
                    unpack: dem_data.get_unpack_vector(),
                    // Use stride (dim + 2) for dimension, as the texture
                    // includes a 1-pixel border
                    dimension: [dem_data.stride as f32, dem_data.stride as f32],
                    color_ramp_size: self.color_ramp_size as i32,
                    pad_tile0: 0.0,
                };
                let generation = bucket.render_target_generation();
                let dem_texture = match self.dem_textures.get(&bucket_id) {
                    Some((texture, made_at)) if *made_at == generation => Rc::clone(texture),
                    _ => {
                        let texture = context.create_texture_2d();
                        texture.borrow_mut().set_image(Some(Rc::new(dem_data.get_image().clone())));
                        self.dem_textures.insert(bucket_id, (Rc::clone(&texture), generation));
                        texture
                    }
                };
                dem_texture.borrow_mut().set_sampler(dem_sampler);
                let mut textures = Textures::default();
                textures[ID_COLOR_RELIEF_IMAGE_TEXTURE] = Some(dem_texture);
                textures[ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE] = Some(Rc::clone(&elevation_stops_texture));
                textures[ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE] = Some(Rc::clone(&color_stops_texture));

                if self.drawing.caches.cache_mut(&tile_id).is_some_and(|cached| cached.built < geometry_time) {
                    // The masked geometry changed (see the file comment).
                    self.drawing.caches.drop_cache(&tile_id);
                }
                if let Some(cached) = self.drawing.caches.cache_mut(&tile_id) {
                    // The DEM and the ramps' textures change with the
                    // bucket's generation and the ramps.
                    cached.draw.objects.textures = textures;
                } else {
                    // The tile's geometry: the globe's mesh of its mask, the
                    // mask's geometry, or the whole tile's quad.
                    let (vertices, vertex_count, indices, segments): (
                        VertexVectorBasePtr,
                        usize,
                        IndexVectorBasePtr,
                        Vec<SegmentBase>,
                    ) = if self.base.projection_variant == ProjectionVariant::Globe {
                        let globe_mesh =
                            self.globe_meshes.get(&tile_id.canonical, &geometry.mask, HillshadeBucket::layout_vertex);
                        let vertices: VertexVectorBasePtr = Rc::<
                            mln_gfx::vertex_vector::VertexVector<HillshadeLayoutVertex>,
                        >::clone(&globe_mesh.vertices);
                        (
                            vertices,
                            globe_mesh.vertices.elements(),
                            Rc::clone(&globe_mesh.indices),
                            globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                        )
                    } else if !geometry.vertices.is_empty()
                        && !geometry.indices.is_empty()
                        && !geometry.segments.is_empty()
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
                    };
                    let geometry = DrawGeometry {
                        vertices: color_relief_vertex_attributes(&vertices),
                        vertex_count: if segments.is_empty() { 0 } else { vertex_count },
                        mode: DrawMode::Triangles(),
                        indexes: indices,
                        segments,
                        instances: None,
                    };
                    let objects = DrawObjects::new(
                        Rc::clone(&color_relief_shader),
                        ColorMode::alpha_blended(),
                        CullFaceMode::disabled(),
                        geometry,
                        textures,
                    );
                    let cached = ReliefTile { draw: KeptDraw::new(objects), built: Instant::now(), tile_props };
                    self.drawing.caches.set_cache(&tile_id, cached);
                }
            } else {
                // The tile's draw, if it has one, draws as it was.
                Log::warning(Event::Render, "ColorRelief: DEM image not valid for tile");
            }

            let Some(cached) = self.drawing.caches.cache_mut(&tile_id) else {
                continue;
            };
            let projection = paint_parameters.projection_data_for_tile(&tile_id.to_unwrapped(), false);
            let uniforms = ReliefUniforms {
                draw_ubo: ColorReliefDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) },
                tile_props: cached.tile_props,
                projection: to_projection_ubo(&projection),
            };
            let draw = FrameDraw { depth: DrawDepth::Off, ..cached.draw.frame_draw(Some(tile_id)) };
            frame_draws.push((draw, uniforms));
        }

        frame_draws.sort_by_key(|(draw, _)| draw.order());
        let Some(frame) = &mut self.drawing.frame else {
            return;
        };
        if !frame_draws.is_empty() {
            let props = ColorReliefEvaluatedPropsUBO {
                opacity: *self.values.evaluated.paint.color_relief_opacity.uniform(),
                pad_eval0: 0.0,
                pad_eval1: 0.0,
                pad_eval2: 0.0,
            };
            let buffers = &mut frame.uniform_buffers;
            buffers.create_or_update_pod(ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, &props, context);
            let draw_ubos: Vec<ColorReliefDrawableUBO> =
                frame_draws.iter().map(|(_, uniforms)| uniforms.draw_ubo).collect();
            buffers.write_storage(ID_COLOR_RELIEF_DRAWABLE_UBO, bytemuck::cast_slice(&draw_ubos), context);
            let tile_props: Vec<ColorReliefTilePropsUBO> =
                frame_draws.iter().map(|(_, uniforms)| uniforms.tile_props).collect();
            buffers.write_storage(ID_COLOR_RELIEF_TILE_PROPS_UBO, bytemuck::cast_slice(&tile_props), context);
            let projections: Vec<ProjectionUBO> = frame_draws.iter().map(|(_, uniforms)| uniforms.projection).collect();
            buffers.write_storage(ID_PROJECTION_UBO, bytemuck::cast_slice(&projections), context);
        }
        frame.draws.clear();
        frame.draws.extend(frame_draws.into_iter().map(|(draw, _)| draw));
    }
}
