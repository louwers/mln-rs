// A tile's draw is built again only when the tile's mask changed, and the
// layer keeps each DEM's texture and the quads of each mask by the id of the
// tile's contents. The elevation stops texture is made anew when the ramp
// changes, since its size changes with the ramp.

use std::collections::hash_map::Entry;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::StyleValue as _;
use mln_style::bitmap::PremultipliedImage;
use mln_style::color::Color;
use mln_style::expression::{
    Dependency, EvaluationContext, GlobalStateMap, from_expression_value, global_state_refs_intersect,
};
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::LayerStyle;
use mln_style::layers::ColorRelief;
use mln_style::property::color_ramp::ColorRampPropertyValue;
use mln_style::size::Size;
use mln_style::types::ResamplingType;

use super::cast_matrix;
use crate::buckets::raster_bucket::{MaskQuads, RasterLayoutVertex};
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::draw::objects::{DrawGeometry, DrawObjects, Textures};
use crate::draw::segment::SegmentBase;
use crate::draw::store::SharedTexture;
use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::draw::vertices::SharedVertices;
use crate::layer::bucket_geometry::copied_vertices;
use crate::layer::layer_drawing::{DrawList, DrawObjectsMut, KeptDraw, TileDraws, TileObjects};
use crate::layer::layer_type::LayerType;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, LayerUpdateParameters, LayerValues, OnTerrain, Parts, PartsMut, RenderLayer,
    RenderLayerBase, TypedLayer,
};
use crate::layers::color_relief::programs;
use crate::layers::color_relief::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use crate::log_target;
use crate::render::globe::globe_tile_mesh::GlobeTileMeshCache;
use crate::render::paint_parameters::PaintParameters;
use crate::render::static_data::RenderStaticData;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::to_projection_ubo;
use crate::source::DemTiles;
use crate::tile::dem::DemUnpack;

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::COLOR_RELIEF.buffer_id("drawableVector");
const TILE_PROPS_ID: usize = programs::COLOR_RELIEF.buffer_id("tilePropsVector");
const PROPS_ID: usize = programs::COLOR_RELIEF.buffer_id("props");

/// The slots of the programs' textures that the draws bind.
const COLOR_STOPS_SLOT: usize = programs::COLOR_RELIEF.texture_slot("color_stops");
const ELEVATION_STOPS_SLOT: usize = programs::COLOR_RELIEF.texture_slot("elevation_stops");
const IMAGE_SLOT: usize = programs::COLOR_RELIEF.texture_slot("image");

/// What the layer keeps of a tile: the draw that colors its DEM, with the
/// generation of the tile's mask and the DEM's properties it was built with.
pub(crate) struct ReliefTile {
    draw: KeptDraw,
    geometry_generation: u64,
    tile_props: ColorReliefTilePropsUBO,
}

impl TileObjects for ReliefTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        self.draw.objects(())
    }

    fn drop_bindings(&mut self) {
        self.draw.drop_bindings();
    }
}

/// A draw's entries in the layer's storage buffers.
struct ReliefUniforms {
    draw_ubo: ColorReliefDrawableUBO,
    tile_props: ColorReliefTilePropsUBO,
    projection: ProjectionUBO,
}

impl LayerType for ColorRelief {
    type Input = DemTiles;

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

    static_data_shared_vertices: Option<SharedVertices>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<SharedIndexes>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// The texture of each tile's DEM, by the id of its contents, made from
    /// its current, possibly backfilled, image, with the generation of the
    /// DEM's border it was made at.
    dem_textures: HashMap<SimpleIdentity, (SharedTexture, u64)>,
    /// The quads of each tile's mask, by the id of its contents.
    mask_quads: MaskQuads,
    /// The contents of the frame's tiles, whose DEM textures and quads stay.
    live_dems: HashSet<SimpleIdentity>,
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
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            dem_textures: HashMap::default(),
            mask_quads: MaskQuads::default(),
            live_dems: HashSet::default(),
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

        // The ramps at their new size, as RGBA floats and colors, keeping
        // what fits of the last.
        self.elevation_stops_data.resize(ramp_size as usize * 4, 0.0);
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

impl TypedLayer for RenderColorReliefLayer {
    type Type = ColorRelief;
    type Drawing = TileDraws<ReliefTile>;
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

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let parameters = cx.parameters;
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
        let evaluated = &*cx.evaluated;
        if *evaluated.paint.color_relief_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        render_tiles: &[FrameTile<'_, ColorRelief>],
    ) {
        let RenderColorReliefLayer {
            base,
            values,
            drawing,
            draw_list,
            color_ramp_size,
            color_ramp_changed,
            elevation_stops_data,
            color_stops,
            elevation_stops_texture,
            color_stops_texture,
            static_data_shared_vertices,
            static_data_indices,
            globe_meshes,
            dem_textures,
            mask_quads,
            ..
        } = self;

        let color_relief_shader =
            paint_parameters.context.shader_key(&programs::COLOR_RELIEF, Uniforms::default(), base.projection_variant);

        let context = &mut *paint_parameters.context;
        let static_data_shared_vertices = &*static_data_shared_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices()));
        let static_data_indices = &*static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())));

        // Update color ramp textures if changed
        if *color_ramp_changed {
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
            let data = bytemuck::cast_slice(elevation_stops_data.as_slice());
            if let Err(error) =
                context.upload_texture_data(gpu, &elevation_stops, data, mln_gfx::Size::new(*color_ramp_size, 1))
            {
                context.fail(format!("Uploading the elevation stops failed: {error}"));
            }
            *elevation_stops_texture = Some(elevation_stops);

            let texture = color_stops_texture.get_or_insert_with(|| context.add_texture());
            if let Some(mut texture) = context.texture_mut(texture) {
                texture.set_image(Some(Rc::new(color_stops.clone())));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            *color_ramp_changed = false;
        }

        // Skip rendering if color ramp textures aren't ready
        let (Some(elevation_stops_texture), Some(color_stops_texture)) =
            (elevation_stops_texture.as_ref(), color_stops_texture.as_ref())
        else {
            return;
        };

        let dem_sampler = SamplerState {
            filter: match *values.evaluated.paint.resampling.uniform() {
                ResamplingType::Linear => TextureFilterType::Linear,
                ResamplingType::Nearest => TextureFilterType::Nearest,
            },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        };

        for tile in render_tiles {
            let tile_id = tile.draw_id();
            let Some(mut entry) = drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let Some(dem) = tile.data().filter(|dem| dem.has_data()) else {
                *entry.value = None;
                continue;
            };
            let dem_id = dem.id();
            // A draw built from other contents goes.
            entry.set_bucket(dem_id);

            let masking = &dem.masking;
            let geometry_generation = masking.generation();
            let dem_data = dem.dem();
            if dem_data.image().valid() {
                let unpack = DemUnpack::of(dem_data.encoding);
                let tile_props = ColorReliefTilePropsUBO {
                    unpack: unpack.weights_and_base(),
                    // Use stride (dim + 2) for dimension, as the texture
                    // includes a 1-pixel border
                    dimension: [dem_data.stride as f32, dem_data.stride as f32],
                    color_ramp_size: *color_ramp_size as i32,
                    unpack_scale: unpack.scale,
                };
                let generation = dem.generation();
                let context = &mut *paint_parameters.context;
                let (dem_texture, _) = match dem_textures.entry(dem_id) {
                    Entry::Occupied(entry) if entry.get().1 == generation => entry.into_mut(),
                    entry => {
                        let texture = context.add_texture_with(|texture| {
                            texture.set_image(Some(Rc::new(dem_data.image().clone())));
                        });
                        entry.insert_entry((texture, generation)).into_mut()
                    }
                };
                if let Some(mut texture) = context.texture_mut(dem_texture) {
                    texture.set_sampler(dem_sampler);
                }
                let textures = [
                    (IMAGE_SLOT, &*dem_texture),
                    (ELEVATION_STOPS_SLOT, elevation_stops_texture),
                    (COLOR_STOPS_SLOT, color_stops_texture),
                ];

                if entry.value.as_ref().is_some_and(|cached| cached.geometry_generation != geometry_generation) {
                    // The mask changed (see the file comment).
                    *entry.value = None;
                }
                if let Some(cached) = entry.value.as_mut() {
                    // The DEM and the ramps' textures change with the
                    // DEM's generation and the ramps.
                    for (slot, texture) in textures {
                        cached.draw.objects.set_texture(slot, Some(texture));
                    }
                } else {
                    // The tile's geometry: the globe's mesh of its mask, the
                    // mask's quads, or the whole tile's quad.
                    let (vertices, vertex_count, indices, segments): (
                        SharedVertices,
                        usize,
                        DrawIndexes,
                        Vec<SegmentBase>,
                    ) = if base.projection_variant == ProjectionVariant::Globe {
                        let globe_mesh =
                            globe_meshes.get(context, &tile_id.canonical, masking.mask(), RasterLayoutVertex::new);
                        (
                            globe_mesh.vertices.clone(),
                            globe_mesh.vertices.count(),
                            globe_mesh.indexes(),
                            globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                        )
                    } else if let Some(quads) = mask_quads.get(context, dem_id, masking) {
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
                    };
                    let geometry = DrawGeometry {
                        vertices: color_relief_vertex_attributes(&vertices),
                        vertex_count: if segments.is_empty() { 0 } else { vertex_count },
                        mode: DrawMode::Triangles(),
                        indexes: indices,
                        segments,
                        instances: None,
                    };
                    let mut draw_textures = Textures::default();
                    for (slot, texture) in textures {
                        draw_textures[slot] = Some(texture.clone());
                    }
                    let objects = DrawObjects::new(
                        color_relief_shader.clone(),
                        ColorMode::alpha_blended(),
                        CullFaceMode::disabled(),
                        geometry,
                        draw_textures,
                    );
                    let cached = ReliefTile { draw: KeptDraw::new(objects), geometry_generation, tile_props };
                    *entry.value = Some(cached);
                }
            } else {
                // The tile's draw, if it has one, draws as it was.
                log::warn!(target: log_target::RENDER, "ColorRelief: DEM image not valid for tile");
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
            let draw = cached.draw.frame_draw(Some(tile_id));
            draw_list.push(draw, uniforms);
        }

        let opacity = *values.evaluated.paint.color_relief_opacity.uniform();
        drawing.finish(draw_list, gpu, paint_parameters.context, |writer| {
            let props = ColorReliefEvaluatedPropsUBO { opacity, pad_eval0: 0.0, pad_eval1: 0.0, pad_eval2: 0.0 };
            writer.uniform(PROPS_ID, &props);
            writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
            writer.storage(TILE_PROPS_ID, |uniforms| uniforms.tile_props);
            writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
        });
    }

    /// Drops the DEM textures and quads of the tiles the frame doesn't draw,
    /// in every frame, including one the layer draws nothing in.
    fn update(&mut self, _params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, ColorRelief>]) {
        let live_dems = &mut self.live_dems;
        live_dems.clear();
        live_dems.extend(tiles.iter().filter_map(|tile| Some(tile.data()?.id())));
        self.dem_textures.retain(|id, _| live_dems.contains(id));
        self.mask_quads.retain(live_dems);
    }

    fn release(&mut self) {
        self.dem_textures.clear();
        self.mask_quads.clear();
    }
}

impl RenderLayer for RenderColorReliefLayer {}
