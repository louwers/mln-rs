// A tile's draw is built again only when the bucket's masked geometry
// changed, and the layer keeps each bucket's DEM texture by its id. The
// elevation stops texture is made anew when the ramp changes, since its size
// changes with the ramp.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::TextureFormat;
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
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, ShaderProgramPtr};
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

use super::cast_matrix;
use crate::bucket::NoBucket;
use crate::bucket_geometry::released_vertices;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterLayoutVertex;
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::gfx::store::SharedTexture;
use crate::gfx::vertex_attribute::VertexAttributeArray;
use crate::gfx::vertices::SharedVertices;
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, KeptDraw, TileDraws, TileObjects};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerParts, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::tile_matrix::to_projection_ubo;

const COLOR_RELIEF_SHADER_GROUP_NAME: &str = "ColorReliefShader";

/// What the layer keeps of a tile: the draw that colors its bucket's DEM,
/// with when it was built and the DEM's properties it was built with.
pub(crate) struct ReliefTile {
    draw: KeptDraw,
    built: Instant,
    tile_props: ColorReliefTilePropsUBO,
}

impl TileObjects for ReliefTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        self.draw.objects(())
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
    type TileBucket<'a> = &'a HillshadeBucket;

    fn render_layer(style: Arc<LayerStyle<ColorRelief>>) -> Box<dyn RenderLayer> {
        Box::new(RenderColorReliefLayer::new(style))
    }
}

pub(crate) struct RenderColorReliefLayer {
    base: RenderLayerBase,
    values: LayerValues<ColorRelief>,
    drawing: TileDraws<ReliefTile>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<ReliefUniforms>,

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
    elevation_stops_texture: Option<SharedTexture>,
    color_stops_texture: Option<SharedTexture>,

    color_relief_shader: Option<ShaderProgramPtr>,

    static_data_shared_vertices: Option<SharedVertices>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<SharedIndexes>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The DEM texture of each bucket, made from its current, possibly
    /// backfilled, DEM image, with the bucket's prepare generation it was
    /// made at.
    dem_textures: HashMap<SimpleIdentity, (SharedTexture, u64)>,
    /// The buckets of the frame's tiles, whose DEM textures stay.
    live_buckets: HashSet<SimpleIdentity>,
}

impl RenderColorReliefLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<ColorRelief>>) -> Self {
        let (base, values) = LayerValues::new(impl_);

        // Initialize color ramp data with RGBA format (4 floats per stop)
        let color_ramp_size = 256;
        let mut layer = RenderColorReliefLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            color_ramp_size,
            color_ramp_changed: true,
            color_ramp_global_state: None,
            elevation_stops_data: vec![0.0; color_ramp_size as usize * 4],
            color_stops: PremultipliedImage::new(Size::new(color_ramp_size, 1)),
            elevation_stops_texture: None,
            color_stops_texture: None,
            color_relief_shader: None,
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            dem_textures: HashMap::default(),
            live_buckets: HashSet::default(),
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

        let Some(expr) = color_value.get_expression() else {
            return;
        };

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
fn color_relief_vertex_attributes(vertices: &SharedVertices) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let stride = size_of::<RasterLayoutVertex>() as u32;
    if let Some(attr) = attrs.set(ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, AttributeDataType::Invalid) {
        let offset = offset_of!(RasterLayoutVertex, a1) as u32;
        attr.set_shared_vertices(vertices.clone(), offset, 0, stride, AttributeDataType::Short2);
    }
    if let Some(attr) = attrs.set(ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, AttributeDataType::Invalid) {
        let offset = offset_of!(RasterLayoutVertex, a2) as u32;
        attr.set_shared_vertices(vertices.clone(), offset, 0, stride, AttributeDataType::Short2);
    }
    attrs
}

impl TypedLayer for RenderColorReliefLayer {
    type Type = ColorRelief;
    type Drawing = TileDraws<ReliefTile>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Drops the shader of the last projection, and the globe's meshes
    /// when the frame isn't on the globe.
    fn projection_changed(&mut self) {
        self.color_relief_shader = None;
        if self.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
    }

    fn complete_evaluation(
        &mut self,
        parameters: &PropertyEvaluationParameters,
        _evaluated: &mut EvaluatedLayer<ColorRelief>,
    ) {
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
    }

    fn passes(&self, evaluated: &EvaluatedLayer<ColorRelief>) -> RenderPass {
        if *evaluated.paint.color_relief_opacity.uniform() > 0.0 { RenderPass::Translucent } else { RenderPass::None }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn update_draws(
        &mut self,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, ColorRelief>],
    ) {
        let shaders = &paint_parameters.static_data.shaders;

        if self.color_relief_shader.is_none() {
            self.color_relief_shader = shaders.shader(COLOR_RELIEF_SHADER_GROUP_NAME, self.base.projection_variant);
        }
        let Some(color_relief_shader) = self.color_relief_shader.clone() else {
            Log::error(Event::Render, "ColorRelief shader failed to load");
            self.remove_all_draws();
            return;
        };

        let context = &mut *paint_parameters.context;
        let static_data_shared_vertices = self
            .static_data_shared_vertices
            .get_or_insert_with(|| released_vertices(context, &RenderStaticData::raster_vertices()))
            .clone();
        let static_data_indices = self
            .static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())))
            .clone();

        // Update color ramp textures if changed
        if self.color_ramp_changed {
            let context = &mut *paint_parameters.context;
            // RGBA32F rather than R32F, which llvmpipe can't sample.
            let elevation_stops = context.add_texture_with(|texture| {
                texture.set_format(TextureFormat::Rgba32Float);
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Nearest,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            });
            let data = bytemuck::cast_slice(&self.elevation_stops_data);
            if let Err(error) = context.upload_texture_data(&elevation_stops, data, Size::new(self.color_ramp_size, 1))
            {
                Log::error(Event::Render, format!("Uploading the elevation stops failed: {error}"));
            }
            self.elevation_stops_texture = Some(elevation_stops);

            let color_stops = self.color_stops_texture.get_or_insert_with(|| context.add_texture());
            if let Some(mut texture) = context.texture_mut(color_stops) {
                texture.set_image(Some(Rc::new(self.color_stops.clone())));
                texture.set_sampler(SamplerState {
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
        let live_buckets = &mut self.live_buckets;
        live_buckets.clear();
        live_buckets.extend(render_tiles.iter().filter_map(|tile| Some(tile.bucket()?.base.bucket_id)));
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

        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let Some(bucket) = tile.bucket().filter(|bucket| bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket_id = bucket.base.bucket_id;
            // A draw built from another bucket goes.
            entry.set_bucket(bucket_id);

            let geometry = bucket.geometry();
            let geometry_time = geometry.last_modified;
            let dem_data = bucket.dem_data();
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
                let context = &mut *paint_parameters.context;
                let dem_texture = match self.dem_textures.get(&bucket_id) {
                    Some((texture, made_at)) if *made_at == generation => texture.clone(),
                    _ => {
                        let texture = context.add_texture_with(|texture| {
                            texture.set_image(Some(Rc::new(dem_data.get_image().clone())));
                        });
                        self.dem_textures.insert(bucket_id, (texture.clone(), generation));
                        texture
                    }
                };
                if let Some(mut texture) = context.texture_mut(&dem_texture) {
                    texture.set_sampler(dem_sampler);
                }
                let mut textures = Textures::default();
                textures[ID_COLOR_RELIEF_IMAGE_TEXTURE] = Some(dem_texture);
                textures[ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE] = Some(elevation_stops_texture.clone());
                textures[ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE] = Some(color_stops_texture.clone());

                if entry.value.as_ref().is_some_and(|cached| cached.built < geometry_time) {
                    // The masked geometry changed (see the file comment).
                    *entry.value = None;
                }
                if let Some(cached) = entry.value.as_mut() {
                    // The DEM and the ramps' textures change with the
                    // bucket's generation and the ramps.
                    cached.draw.objects.set_textures(textures);
                } else {
                    // The tile's geometry: the globe's mesh of its mask, the
                    // mask's geometry, or the whole tile's quad.
                    let (vertices, vertex_count, indices, segments): (
                        SharedVertices,
                        usize,
                        DrawIndexes,
                        Vec<SegmentBase>,
                    ) = if self.base.projection_variant == ProjectionVariant::Globe {
                        let globe_mesh =
                            self.globe_meshes.get(context, &tile_id.canonical, &geometry.mask, RasterLayoutVertex::new);
                        (
                            globe_mesh.vertices.clone(),
                            globe_mesh.vertices.count(),
                            globe_mesh.indexes(),
                            globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                        )
                    } else if let Some(quads) = geometry.quads() {
                        (
                            quads.vertices.clone(),
                            geometry.vertex_count(),
                            quads.indexes.clone().into(),
                            geometry.segments.iter().map(SegmentBase::copy).collect(),
                        )
                    } else {
                        (
                            static_data_shared_vertices.clone(),
                            4,
                            static_data_indices.clone().into(),
                            RenderStaticData::raster_segments(),
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
                    *entry.value = Some(cached);
                }
            } else {
                // The tile's draw, if it has one, draws as it was.
                Log::warning(Event::Render, "ColorRelief: DEM image not valid for tile");
            }

            let Some(cached) = entry.value.as_ref() else {
                continue;
            };
            let projection = paint_parameters.projection_data_for_tile(&tile_id.to_unwrapped(), false);
            let uniforms = ReliefUniforms {
                draw_ubo: ColorReliefDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) },
                tile_props: cached.tile_props,
                projection: to_projection_ubo(&projection),
            };
            let draw = FrameDraw { depth: DrawDepth::Off, ..cached.draw.frame_draw(Some(tile_id)) };
            self.draw_list.push(draw, uniforms);
        }

        let frame = self.drawing.frame_mut();
        let opacity = *self.values.evaluated.paint.color_relief_opacity.uniform();
        self.draw_list.finish(frame, paint_parameters.context, |writer| {
            let props = ColorReliefEvaluatedPropsUBO { opacity, pad_eval0: 0.0, pad_eval1: 0.0, pad_eval2: 0.0 };
            writer.uniform(ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, &props);
            writer.storage(ID_COLOR_RELIEF_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
            writer.storage(ID_COLOR_RELIEF_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
            writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
        });
    }

    fn release(&mut self) {
        self.dem_textures.clear();
    }
}

impl RenderLayer for RenderColorReliefLayer {}
