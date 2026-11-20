// Each DEM tile is first drawn into its own render target by the prepare
// shader (slopes from the DEM), then the tile's draw shades that texture.
// The layer keeps the render targets by the tile contents' id
// (`render_targets`), with the DEM's generation, which counts the changes
// of its border, and a tile's draw takes the contents' masked geometry
// again only when it changed.

use std::collections::hash_map::Entry;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_HILLSHADE_DRAWABLE_UBO, ID_HILLSHADE_EVALUATED_PROPS_UBO, ID_HILLSHADE_IMAGE_TEXTURE,
    ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_PREPARE_DRAWABLE_UBO, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
    ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TILE_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_math::deg2radf;
use mln_renderer::layer::{
    DemContents, DrawContext, EvaluatedLayer, LayerType, LayerValues, NoBucket, NoLayout, OffscreenPass,
    RasterLayoutVertex, RenderPass, UpdateContext,
};
use mln_renderer::unstable::{
    Context, DrawGeometry, DrawIndexes, DrawList, DrawObjects, DrawObjectsMut, FrameDraw, GlobeTileMeshCache, Indexes,
    KeptDraw, PaintParameters, RenderStaticData, RenderTarget, SharedAttribute, SharedIndexes, SharedTexture,
    SharedVertices, SingleDraw, Textures, TileDraws, TileObjects, Uniforms, VertexAttributeArray, cast_matrix,
    copied_vertices, projection_data, to_projection_ubo,
};
use mln_style_types::{HillshadeIlluminationAnchorType, HillshadeMethodType, ResamplingType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::constants::{self, EXTENT};
use mln_util::geo::LatLng;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::size::Size;

use super::{Hillshade, HillshadePaintProperties, programs};
use crate::hillshade::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES,
};

/// What the layer keeps of a DEM: the render target its slopes
/// are prepared in, and the draw that prepares them.
struct DemTarget {
    render_target: RenderTarget,
    /// The contents' DEM, which `prepare` samples.
    _dem: SharedTexture,
    /// The draw that prepares the slopes, with the uniforms it reads.
    prepare: SingleDraw,
    /// The DEM's generation the target was made at.
    generation: u64,
}

/// What the layer keeps of a tile: the draw that shades the slopes of its
/// contents, with the generation of the contents' geometry it took.
pub struct HillshadeTile {
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

/// What a hillshade layer keeps besides its drawing.
pub struct HillshadeState {
    /// The frame's draws as the update makes them.
    draw_list: DrawList<HillshadeUniforms>,
    maxzoom: u8,

    static_data_shared_vertices: Option<SharedVertices>,
    /// The triangles of the static data's quad.
    static_data_indices: Option<SharedIndexes>,
    globe_meshes: GlobeTileMeshCache<RasterLayoutVertex>,

    /// Each DEM's render target, by its contents' id. They draw while the layer
    /// is renderable.
    render_targets: HashMap<SimpleIdentity, DemTarget>,
    /// The contents of the frame's tiles, which keep their render targets.
    live_contents: HashSet<SimpleIdentity>,
    /// The illumination of the evaluation the draws last drew with, and its
    /// number of light sources: a viewport anchor's takes the bearing of the
    /// first frame that draws with it.
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
fn hillshade_vertex_attributes(vertices: &SharedVertices) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let stride = size_of::<RasterLayoutVertex>() as u32;
    attrs.set_shared(
        ID_HILLSHADE_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a1) as u32,
            stride,
        ),
    );
    attrs.set_shared(
        ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(
            vertices.clone(),
            AttributeDataType::Short2,
            offset_of!(RasterLayoutVertex, a2) as u32,
            stride,
        ),
    );
    attrs
}

impl LayerType for Hillshade {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = &'a DemContents;

    type Drawing = TileDraws<HillshadeTile>;
    type State = HillshadeState;

    fn new_layer(_layer: &LayerValues<Hillshade>) -> (HillshadeState, TileDraws<HillshadeTile>) {
        let state = HillshadeState {
            draw_list: DrawList::default(),
            maxzoom: constants::TERRAIN_RGB_MAXZOOM,
            static_data_shared_vertices: None,
            static_data_indices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            render_targets: HashMap::default(),
            live_contents: HashSet::default(),
            illumination: None,
        };
        (state, TileDraws::default())
    }

    /// Drops the globe's meshes when the frame isn't on the globe.
    fn projection_changed(state: &mut HillshadeState, variant: ProjectionVariant) {
        if variant == ProjectionVariant::Mercator {
            state.globe_meshes.clear();
        }
    }

    fn passes(_layer: &LayerValues<Hillshade>, evaluated: &EvaluatedLayer<Hillshade>) -> RenderPass {
        if *evaluated.paint.hillshade_exaggeration.uniform() > 0.0 { RenderPass::TRANSLUCENT } else { RenderPass::NONE }
    }

    /// Makes the render target of each DEM it has none of, which prepares
    /// its slopes, and the frame's draws of the layer, those of each of its
    /// tiles, which shade the slopes and which it builds for a tile it keeps
    /// none of, and writes the layer's buffers they read.
    fn make_draws(
        state: &mut HillshadeState,
        drawing: &mut TileDraws<HillshadeTile>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, '_, Hillshade>,
    ) {
        let (paint_parameters, render_tiles) = (&mut *cx.paint, cx.tiles);
        let context = &mut *paint_parameters.context;
        let hillshade_prepare_shader =
            context.shader_key(&programs::HILLSHADE_PREPARE, Uniforms::default(), ProjectionVariant::Mercator);
        let hillshade_shader = context.shader_key(&programs::HILLSHADE, Uniforms::default(), cx.projection_variant);
        let color_mode = context.color_mode(ColorMode::alpha_blended());
        let static_data_shared_vertices = &*state
            .static_data_shared_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::raster_vertices()));
        let static_data_indices = &*state
            .static_data_indices
            .get_or_insert_with(|| context.add_indexes(Indexes::new(RenderStaticData::quad_triangle_indices())));

        // `resampling` filters the prepared texture the tile's draw samples;
        // the prepare pass reads the DEM with Nearest regardless.
        let resampling_filter = match *cx.layer.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };

        // Drop the render targets of contents that are no longer rendered.
        let live_contents = &mut state.live_contents;
        live_contents.clear();
        live_contents.extend(render_tiles.iter().filter_map(|tile| Some(tile.view()?.id())));
        state.render_targets.retain(|id, _| live_contents.contains(id));

        // The illumination, which the first draw computes.
        let mut illumination = None;
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

            let generation = contents.dem_generation();
            let target = match state.render_targets.entry(contents_id) {
                Entry::Occupied(entry) if entry.get().generation == generation => entry.into_mut(),
                entry => {
                    let attrs = hillshade_vertex_attributes(static_data_shared_vertices);
                    let target = DemTarget::new(
                        gpu,
                        paint_parameters.context,
                        contents,
                        &tile_id.to_unwrapped(),
                        state.maxzoom,
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

            let geometry = contents.geometry();
            let geometry_generation = geometry.generation();
            // The tile's geometry: the globe's mesh of its mask, the mask's
            // geometry, or the whole tile's quad.
            let mut tile_geometry = || -> (SharedVertices, usize, DrawIndexes, Vec<SegmentBase>) {
                if cx.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        state.globe_meshes.get(context, &tile_id.canonical, geometry.mask(), RasterLayoutVertex::new);
                    (
                        globe_mesh.vertices.clone(),
                        globe_mesh.vertices.count(),
                        globe_mesh.indexes(),
                        globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                    )
                } else if let Some(quads) = geometry.quads() {
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
                        RenderStaticData::quad_triangle_segments(),
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
                cached.draw.objects.set_texture(ID_HILLSHADE_IMAGE_TEXTURE, Some(render_target_texture));
                cached
            } else {
                let mut textures = Textures::default();
                textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(render_target_texture.clone());
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
                HillshadeState::illumination(&mut state.illumination, &cx.layer.evaluated, paint_parameters)
            });
            let uniforms = HillshadeState::uniforms(
                &cx.layer.evaluated.paint,
                &tile_id.to_unwrapped(),
                num_lights,
                paint_parameters,
            );
            state.draw_list.push(draw, uniforms);
        }

        drawing.finish(&mut state.draw_list, gpu, paint_parameters.context, |writer| {
            if let Some(Illumination { props, .. }) = &illumination {
                writer.uniform(ID_HILLSHADE_EVALUATED_PROPS_UBO, props);
            }
            writer.storage(ID_HILLSHADE_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
            writer.storage(ID_HILLSHADE_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
            writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
        });
    }

    /// Keeps the source's maximum zoom, which the DEMs' preparation reads.
    fn update(
        state: &mut HillshadeState,
        _drawing: &mut TileDraws<HillshadeTile>,
        cx: &mut UpdateContext<'_, '_, Hillshade>,
    ) {
        if let Some(max_zoom) = cx.source_max_zoom() {
            state.maxzoom = max_zoom;
        }
    }

    fn release(state: &mut HillshadeState) {
        state.render_targets.clear();
    }

    fn offscreen_pass<'a>(
        state: &'a mut HillshadeState,
        _drawing: &'a mut TileDraws<HillshadeTile>,
    ) -> Option<&'a mut dyn OffscreenPass> {
        Some(state)
    }
}

/// The draws that prepare the DEMs' render targets.
impl OffscreenPass for HillshadeState {
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for target in self.render_targets.values_mut() {
            target.prepare.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        for DemTarget { render_target, prepare, .. } in self.render_targets.values_mut() {
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

impl DemTarget {
    /// The render target of the slopes of `contents`' DEM, which the draw of `shader`
    /// prepares from its DEM, drawn over `attrs` with the `quad` of the
    /// static data, with the uniforms of `tile_id`, the tile it is made for,
    /// of a source of `maxzoom`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's GPU and context, and the tile's contents, source, program and geometry"
    )]
    fn new(
        gpu: &mut dyn Gpu,
        context: &mut Context,
        contents: &DemContents,
        tile_id: &UnwrappedTileID,
        maxzoom: u8,
        shader: &ShaderKey,
        attrs: VertexAttributeArray,
        indexes: &SharedIndexes,
    ) -> DemTarget {
        let (tilesize, stride, unpack, dem_image) = {
            let dem = contents.dem();
            (dem.dim() as u16, dem.stride(), dem.unpack_vector(), dem.image().clone())
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
            segments: RenderStaticData::quad_triangle_segments(),
            instances: None,
        };
        let mut textures = Textures::default();
        textures[ID_HILLSHADE_IMAGE_TEXTURE] = Some(dem.clone());
        let objects =
            DrawObjects::new(shader.clone(), ColorMode::unblended(), CullFaceMode::disabled(), geometry, textures);
        let mut prepare = SingleDraw::new(objects, FrameDraw::new(None, SimpleIdentity::new()));
        let extent = f64::from(EXTENT);
        let m =
            orthographic(0.0, extent, -extent, 0.0, -1.0, 1.0) * DMat4::from_translation(DVec3::new(0.0, -extent, 0.0));
        prepare.bind_no_projection(gpu, context);
        let draw_ubo = HillshadePrepareDrawableUBO { matrix: cast_matrix(&m) };
        prepare.write_pod(gpu, ID_HILLSHADE_PREPARE_DRAWABLE_UBO, &draw_ubo, context);
        let tile_props = HillshadePrepareTilePropsUBO {
            unpack,
            dimension: [stride as f32, stride as f32],
            zoom: f32::from(tile_id.canonical.z),
            maxzoom: f32::from(maxzoom),
        };
        prepare.write_pod(gpu, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO, &tile_props, context);

        DemTarget { render_target, _dem: dem, prepare, generation: contents.dem_generation() }
    }
}

impl HillshadeState {
    /// The entries in the layer's storage buffers of the draw of `tile_id`
    /// with the layer's values `evaluated`, lit by `num_lights` light
    /// sources.
    fn uniforms(
        evaluated: &HillshadePaintProperties::PossiblyEvaluated,
        tile_id: &UnwrappedTileID,
        num_lights: usize,
        parameters: &PaintParameters<'_>,
    ) -> HillshadeUniforms {
        let projection =
            projection_data(tile_id, parameters, [0.0, 0.0], TranslateAnchorType::Viewport, false, false, None, true);
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
            let bearing = parameters.view.angle() as f32;
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
