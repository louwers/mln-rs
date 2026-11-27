// A tile's draw, with the quads of the tile's mask, is built again when the
// tile's mask or its DEM's border changes, and the layer keeps each DEM's
// texture by the id of the tile's contents, which outlives the draws that a
// new mask builds again. The elevation stops texture is made anew when the
// ramp changes, since its size changes with the ramp.

use std::collections::hash_map::Entry;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::projection_variant::ProjectionVariant;
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
use crate::buckets::raster_bucket::{RasterLayoutVertex, StoredQuads};
use crate::draw::context::Context;
use crate::draw::draws::{Draw, Draws};
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
use crate::layer::tile_input::TileData;
use crate::layers::color_relief::programs;
use crate::layers::color_relief::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use crate::render::globe::globe_tile_mesh::GlobeTileMeshCache;
use crate::render::static_data::RenderStaticData;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::to_projection_ubo;
use crate::source::DemTiles;
use crate::tile::dem::DemUnpack;
use crate::tile_store::dem::DemContents;

/// The slots of the programs' textures that the draws bind.
const COLOR_STOPS_SLOT: usize = programs::COLOR_RELIEF.texture_slot("color_stops");
const ELEVATION_STOPS_SLOT: usize = programs::COLOR_RELIEF.texture_slot("elevation_stops");
const IMAGE_SLOT: usize = programs::COLOR_RELIEF.texture_slot("image");

/// What the layer keeps of a tile: the mesh that colors its DEM, with the
/// DEM's properties it was built with.
pub(crate) struct ReliefTile {
    mesh: Mesh,
    tile_props: ColorReliefTilePropsUBO,
}

impl LayerType for ColorRelief {
    type Input = DemTiles;

    fn render_layer(style: Arc<LayerStyle<ColorRelief>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderColorReliefLayer>::boxed(style)
    }
}

pub(crate) struct RenderColorReliefLayer {
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
    /// The contents of the frame's tiles, whose DEM textures stay.
    live_dems: HashSet<SimpleIdentity>,
}

impl RenderColorReliefLayer {
    /// Makes the ramps of the layer's `values`.
    fn update_color_ramp(&mut self, values: &LayerValues<ColorRelief>) {
        // Get the color property value, using default if undefined
        let mut color_value = values.unevaluated.color_relief_color.get_value().clone();
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

    /// Makes the ramps' textures again if the ramps changed since.
    fn update_ramp_textures(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if !self.color_ramp_changed {
            return;
        }
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
        let data = bytemuck::cast_slice(self.elevation_stops_data.as_slice());
        if let Err(error) =
            context.upload_texture_data(gpu, &elevation_stops, data, mln_gfx::Size::new(self.color_ramp_size, 1))
        {
            context.fail(format!("Uploading the elevation stops failed: {error}"));
        }
        self.elevation_stops_texture = Some(elevation_stops);

        let texture = self.color_stops_texture.get_or_insert_with(|| context.add_texture());
        if let Some(mut texture) = context.texture_mut(texture) {
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
}

/// The texture of `dem`'s image in `dem_textures`, which this makes where it
/// has none made at the DEM's border's generation.
fn dem_texture<'a>(
    dem_textures: &'a mut HashMap<SimpleIdentity, (SharedTexture, u64)>,
    context: &mut Context,
    dem: &DemContents,
) -> &'a SharedTexture {
    let generation = dem.generation();
    let (dem_texture, _) = match dem_textures.entry(dem.id()) {
        Entry::Occupied(entry) if entry.get().1 == generation => entry.into_mut(),
        entry => {
            let texture = context.add_texture_with(|texture| {
                texture.set_image(Some(Rc::new(dem.dem().image().clone())));
            });
            entry.insert_entry((texture, generation)).into_mut()
        }
    };
    dem_texture
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
    type Tile = ReliefTile;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;

    fn new(values: &LayerValues<ColorRelief>) -> Self {
        // Initialize color ramp data with RGBA format (4 floats per stop)
        let color_ramp_size = 256;
        let mut layer = RenderColorReliefLayer {
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
            live_dems: HashSet::default(),
        };

        // Initialize with default color ramp immediately to avoid
        // uninitialized state
        layer.update_color_ramp(values);
        layer
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
        let ramp_value = cx.values.unevaluated.color_relief_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp(cx.values);
        }
        let evaluated = &*cx.evaluated;
        if *evaluated.paint.color_relief_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// The draw that colors a tile's DEM with the ramps; none for a tile
    /// without a DEM to draw, or before the ramps have textures.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, ColorRelief>,
        dem: TileData<'_, ColorRelief>,
    ) -> Option<ReliefTile> {
        if !dem.has_data() {
            return None;
        }
        let context = &mut *cx.context;
        self.update_ramp_textures(cx.gpu, context);
        let RenderColorReliefLayer {
            color_ramp_size,
            elevation_stops_texture,
            color_stops_texture,
            static_data_shared_vertices,
            static_data_indices,
            globe_meshes,
            dem_textures,
            ..
        } = self;

        let color_relief_shader =
            context.shader_key(&programs::COLOR_RELIEF, Uniforms::default(), cx.base.projection_variant);
        let static_data_shared_vertices = &*static_data_shared_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices()));
        let static_data_indices = &*static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())));
        let (Some(elevation_stops_texture), Some(color_stops_texture)) =
            (elevation_stops_texture.as_ref(), color_stops_texture.as_ref())
        else {
            return None;
        };

        let tile_id = tile.draw_id();
        let masking = &dem.masking;
        let dem_data = dem.dem();
        let unpack = DemUnpack::of(dem_data.encoding);
        let tile_props = ColorReliefTilePropsUBO {
            unpack: unpack.weights_and_base(),
            // Use stride (dim + 2) for dimension, as the texture includes a
            // 1-pixel border
            dimension: [dem_data.stride as f32, dem_data.stride as f32],
            color_ramp_size: *color_ramp_size as i32,
            unpack_scale: unpack.scale,
        };
        let dem_texture = dem_texture(dem_textures, context, dem);

        // The tile's geometry: the globe's mesh of its mask, the mask's
        // quads, or the whole tile's quad.
        let (vertices, vertex_count, indices, segments): (SharedVertices, usize, DrawIndexes, Vec<SegmentBase>) =
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
        draw_textures[IMAGE_SLOT] = Some(dem_texture.clone());
        draw_textures[ELEVATION_STOPS_SLOT] = Some(elevation_stops_texture.clone());
        draw_textures[COLOR_STOPS_SLOT] = Some(color_stops_texture.clone());
        let mesh = Mesh::new(
            context,
            color_relief_shader,
            ColorMode::alpha_blended(),
            CullFaceMode::disabled(),
            geometry,
            draw_textures,
        );
        Some(ReliefTile { mesh, tile_props })
    }

    /// Pushes the frame's draws of the layer, one of each of its tiles.
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, ColorRelief>],
        out: &mut Draws,
    ) {
        if cx.base.projection_variant == ProjectionVariant::Mercator {
            self.globe_meshes.clear();
        }
        self.update_ramp_textures(cx.gpu, cx.context);
        let RenderColorReliefLayer { elevation_stops_texture, color_stops_texture, dem_textures, .. } = self;

        // Skip rendering if color ramp textures aren't ready
        let (Some(elevation_stops_texture), Some(color_stops_texture)) =
            (elevation_stops_texture.as_ref(), color_stops_texture.as_ref())
        else {
            return;
        };

        let dem_sampler = SamplerState {
            filter: match *cx.values.evaluated.paint.resampling.uniform() {
                ResamplingType::Linear => TextureFilterType::Linear,
                ResamplingType::Nearest => TextureFilterType::Nearest,
            },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        };

        for tile in render_tiles {
            let tile_id = tile.draw_id();
            let Some(dem) = tile.data().filter(|dem| dem.has_data()) else {
                cx.tiles.drop_value(&tile.id());
                continue;
            };
            let Some(entry) = cx.tiles.entry(&tile.id()) else {
                continue;
            };
            let context = &mut *cx.context;
            let dem_texture = dem_texture(dem_textures, context, dem);
            if let Some(mut texture) = context.texture_mut(dem_texture) {
                texture.set_sampler(dem_sampler);
            }

            let Some(cached) = entry.value.as_mut() else {
                continue;
            };
            // The ramps' textures change with the ramps.
            let textures = [
                (IMAGE_SLOT, dem_texture),
                (ELEVATION_STOPS_SLOT, elevation_stops_texture),
                (COLOR_STOPS_SLOT, color_stops_texture),
            ];
            for (slot, texture) in textures {
                cached.mesh.set_texture(context, slot, Some(texture));
            }
            let projection = cx.frame.projection_data_for_tile(&tile_id.to_unwrapped(), false);
            let drawable = ColorReliefDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) };
            let draw = Draw { order: cached.mesh.seq(), ..Draw::DEFAULT };
            let projection = to_projection_ubo(&projection);
            out.main().push_at(&cached.mesh, tile_id, &projection, draw, (drawable, cached.tile_props));
        }

        let opacity = *cx.values.evaluated.paint.color_relief_opacity.uniform();
        let props = ColorReliefEvaluatedPropsUBO { opacity, pad_eval0: 0.0, pad_eval1: 0.0, pad_eval2: 0.0 };
        out.main().block(&props);
    }

    /// Drops the DEM textures of the tiles the frame doesn't draw, in every
    /// frame, including one the layer draws nothing in.
    fn update(&mut self, _cx: UpdateCx<'_, '_, Self>, tiles: &[FrameTile<'_, ColorRelief>]) {
        let live_dems = &mut self.live_dems;
        live_dems.clear();
        live_dems.extend(tiles.iter().filter_map(|tile| Some(tile.data()?.id())));
        self.dem_textures.retain(|id, _| live_dems.contains(id));
    }

    fn release(&mut self) {
        self.dem_textures.clear();
    }
}
