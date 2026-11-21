// A tile's draw is built again only when the contents' masked geometry
// changed, and the layer keeps each DEM's texture by its contents' id. The
// elevation stops texture is made anew when the ramp changes, since its size
// changes with the ramp.

use std::collections::hash_map::Entry;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_render::draw::{
    DrawGeometry, DrawIndexes, DrawList, DrawObjects, DrawObjectsMut, Indexes, KeptDraw, RenderStaticData,
    SharedAttribute, SharedIndexes, SharedTexture, SharedVertices, Textures, TileDraws, TileObjects, Uniforms,
    VertexAttributeArray, cast_matrix, copied_vertices, to_projection_ubo,
};
use mln_render::layer::{
    DemContents, DrawContext, EvaluateCx, Evaluation, LayerType, LayerValues, NoBucket, NoLayout, RasterLayoutVertex,
};
use mln_style::StyleValue as _;
use mln_style::color_ramp_property_value::ColorRampPropertyValue;
use mln_style::expression::{
    Dependency, EvaluationContext, GlobalStateMap, from_expression_value, global_state_refs_intersect,
};
use mln_style_types::ResamplingType;
use mln_util::Color;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::programs::{
    DRAWABLE, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
    ID_COLOR_RELIEF_IMAGE_TEXTURE, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE,
    PROPS, TILE_PROPS,
};
use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use super::{ColorRelief, programs};
use crate::mesh::GlobeTileMeshCache;

/// What the layer keeps of a tile: the draw that colors its contents' DEM,
/// with the generation of the contents' geometry and the DEM's properties it
/// was built with.
pub struct ReliefTile {
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

/// What a color-relief layer keeps besides its drawing.
pub struct ReliefState {
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

    /// The texture of each DEM, by its contents' id, made from its current,
    /// possibly backfilled, image, with the DEM's generation it was made at.
    dem_textures: HashMap<SimpleIdentity, (SharedTexture, u64)>,
    /// The contents of the frame's tiles, whose DEM textures stay.
    live_contents: HashSet<SimpleIdentity>,
}

impl ReliefState {
    /// What a new layer of `layer`'s values keeps.
    fn new(layer: &LayerValues<ColorRelief>) -> Self {
        // Initialize color ramp data with RGBA format (4 floats per stop)
        let color_ramp_size = 256;
        let mut state = ReliefState {
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
            live_contents: HashSet::default(),
        };

        // Initialize with default color ramp immediately to avoid
        // uninitialized state
        state.update_color_ramp(layer);
        state
    }

    /// Makes the color ramp of `layer`'s `color-relief-color` again.
    fn update_color_ramp(&mut self, layer: &LayerValues<ColorRelief>) {
        // Get the color property value, using default if undefined
        let mut color_value = layer.unevaluated.color_relief_color.get_value().clone();
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
        ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a1) as u32,
            stride,
        ),
    );
    attrs.set_shared(
        ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a2) as u32,
            stride,
        ),
    );
    attrs
}

impl LayerType for ColorRelief {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = &'a DemContents;

    type Drawing = TileDraws<ReliefTile>;
    type State = ReliefState;

    fn new_layer(layer: &LayerValues<ColorRelief>) -> (ReliefState, TileDraws<ReliefTile>) {
        (ReliefState::new(layer), TileDraws::default())
    }

    /// Drops the globe's meshes when the frame isn't on the globe.
    fn projection_changed(state: &mut ReliefState, variant: ProjectionVariant) {
        if variant == ProjectionVariant::Mercator {
            state.globe_meshes.clear();
        }
    }

    /// Remakes the color ramp where its value may have changed.
    fn evaluate(state: &mut ReliefState, cx: EvaluateCx<'_, ColorRelief>) -> Evaluation {
        let (layer, parameters) = (cx.layer, cx.parameters);
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&state.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = layer.unevaluated.color_relief_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        state.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            state.update_color_ramp(layer);
        }
        if *cx.evaluated.paint.color_relief_opacity.uniform() > 0.0 {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn make_draws(
        state: &mut ReliefState,
        drawing: &mut TileDraws<ReliefTile>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, ColorRelief>,
    ) {
        let (frame, render_tiles, values) = (&cx.frame, cx.tiles, cx.layer);
        let ReliefState {
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
            live_contents,
            ..
        } = state;

        let color_relief_shader =
            cx.context.shader_key(&programs::COLOR_RELIEF, Uniforms::default(), cx.projection_variant);

        let context = &mut *cx.context;
        let static_data_shared_vertices = &*static_data_shared_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices()));
        let static_data_indices = &*static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())));

        // Update color ramp textures if changed
        if *color_ramp_changed {
            let context = &mut *cx.context;
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
            if let Err(error) = context.upload_texture_data(gpu, &elevation_stops, data, Size::new(*color_ramp_size, 1))
            {
                Log::error(Event::Render, format!("Uploading the elevation stops failed: {error}"));
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

        // Drop the DEM textures of contents that are no longer rendered.
        live_contents.clear();
        live_contents.extend(render_tiles.iter().filter_map(|tile| Some(tile.view()?.id())));
        dem_textures.retain(|id, _| live_contents.contains(id));

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
            let tile_id = tile.id();
            let Some(mut entry) = drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let Some(contents) = tile.view().filter(|contents| contents.has_data()) else {
                *entry.value = None;
                continue;
            };
            let contents_id = contents.id();
            // A draw built from other contents goes.
            entry.set_bucket(contents_id);

            let geometry = contents.geometry();
            let geometry_generation = geometry.generation();
            let dem_data = contents.dem();
            if dem_data.image().valid() {
                let tile_props = ColorReliefTilePropsUBO {
                    unpack: dem_data.unpack_vector(),
                    // Use stride (dim + 2) for dimension, as the texture
                    // includes a 1-pixel border
                    dimension: [dem_data.stride() as f32, dem_data.stride() as f32],
                    color_ramp_size: *color_ramp_size as i32,
                    pad_tile0: 0.0,
                };
                let generation = contents.dem_generation();
                let context = &mut *cx.context;
                let (dem_texture, _) = match dem_textures.entry(contents_id) {
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
                    (ID_COLOR_RELIEF_IMAGE_TEXTURE, &*dem_texture),
                    (ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, elevation_stops_texture),
                    (ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, color_stops_texture),
                ];

                if entry.value.as_ref().is_some_and(|cached| cached.geometry_generation != geometry_generation) {
                    // The masked geometry changed (see the file comment).
                    *entry.value = None;
                }
                if let Some(cached) = entry.value.as_mut() {
                    // The DEM and the ramps' textures change with the
                    // DEM's generation and the ramps.
                    for (id, texture) in textures {
                        cached.draw.objects.set_texture(id, Some(texture));
                    }
                } else {
                    // The tile's geometry: the globe's mesh of its mask, the
                    // mask's geometry, or the whole tile's quad.
                    let (vertices, vertex_count, indices, segments): (
                        SharedVertices,
                        usize,
                        DrawIndexes,
                        Vec<SegmentBase>,
                    ) = if cx.projection_variant == ProjectionVariant::Globe {
                        let globe_mesh =
                            globe_meshes.get(context, &tile_id.canonical, geometry.mask(), RasterLayoutVertex::new);
                        (
                            globe_mesh.vertices.clone(),
                            globe_mesh.vertices.count(),
                            globe_mesh.indexes(),
                            globe_mesh.segments.clone(),
                        )
                    } else if let Some(quads) = geometry.quads() {
                        (
                            quads.vertices.clone(),
                            quads.vertices.count(),
                            quads.indexes.clone().into(),
                            quads.segments.clone(),
                        )
                    } else {
                        (
                            static_data_shared_vertices.clone(),
                            4,
                            static_data_indices.clone().into(),
                            RenderStaticData::quad_triangle_segments(),
                        )
                    };
                    let geometry = DrawGeometry {
                        vertices: color_relief_vertex_attributes(&vertices),
                        vertex_count: if segments.is_empty() { 0 } else { vertex_count },
                        mode: DrawMode::Triangles,
                        indexes: indices,
                        segments,
                        instances: None,
                    };
                    let mut draw_textures = Textures::default();
                    for (id, texture) in textures {
                        draw_textures[id] = Some(texture.clone());
                    }
                    let objects = DrawObjects::new(
                        color_relief_shader.clone(),
                        context.color_mode(ColorMode::alpha_blended()),
                        CullFaceMode::disabled(),
                        geometry,
                        draw_textures,
                    );
                    let cached = ReliefTile { draw: KeptDraw::new(objects), geometry_generation, tile_props };
                    *entry.value = Some(cached);
                }
            } else {
                // The tile's draw, if it has one, draws as it was.
                Log::warning(Event::Render, "ColorRelief: DEM image not valid for tile");
            }

            let Some(cached) = entry.value.as_ref() else {
                continue;
            };
            let projection = frame.projection_data_for_tile(&tile_id.to_unwrapped(), false);
            let uniforms = ReliefUniforms {
                draw_ubo: ColorReliefDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) },
                tile_props: cached.tile_props,
                projection: to_projection_ubo(&projection),
            };
            let draw = cached.draw.frame_draw(Some(tile_id));
            draw_list.push(draw, uniforms);
        }

        let opacity = *values.evaluated.paint.color_relief_opacity.uniform();
        drawing.finish(draw_list, gpu, cx.context, |writer| {
            let props = ColorReliefEvaluatedPropsUBO { opacity, pad_eval0: 0.0, pad_eval1: 0.0, pad_eval2: 0.0 };
            writer.uniform(PROPS.id, &props);
            writer.storage(DRAWABLE.id, |uniforms| uniforms.draw_ubo);
            writer.storage(TILE_PROPS.id, |uniforms| uniforms.tile_props);
            writer.storage(PROJECTION_BUFFER.id, |uniforms| uniforms.projection);
        });
    }

    fn release(state: &mut ReliefState) {
        state.dem_textures.clear();
    }
}
