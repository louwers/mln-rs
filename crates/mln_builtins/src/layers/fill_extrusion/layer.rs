// Each tile draws its roofs from the bucket's triangles and its walls as
// instances of one static quad, an instance per bucket vertex. The walls
// also read the next vertex as attributes of their own (`next_outline_pos`
// and `next_decimals_ed`), from the same buffer one vertex further on, so a
// tile draws an instance for every vertex but the last, which ends a ring and
// has no wall.
//
// A device that rasterizes on the CPU runs its vertex pipeline once per
// instance, so there each wall is a quad of its own in the vertex buffer:
// every outline vertex is in it four times, once with each corner of the
// quad (`FillExtrusionWallVertex`), and a wall reads the next outline vertex
// from the same buffer four vertices further on. The wall shaders run the
// instanced ones' code with every attribute per vertex: the same vertices in
// the same order, drawn without instances.
//
// A device that binds vertex buffers for shaders to read as storage buffers
// draws four vertices a wall without attributes: the shader reads the wall's
// outline vertex, the next one and its paint values from the buffers the
// roofs read, by the vertex's index. A layer with a pattern draws instances
// there too.
//
// Over terrain, each extrusion stands on the ground under its polygon's
// centroid: the tile keeps that height for each of its bucket's vertices,
// which the roofs and instanced walls read as an attribute, the walls'
// quads in a copy four times over, and the pulled walls by index.
//
// A translucent layer blends only its nearest surface over each pixel. Where
// the layer composite's texture can draw with the frame's depth, the layer
// draws into it once, opaque, and composites at its opacity. Elsewhere, and
// with a pattern, whose texels may be translucent themselves, it draws into
// the frame's depth first and then in color where that depth is its own.

use std::mem::offset_of;

use mln_engine::camera::View;
use mln_engine::draw::ground::{FrameGround, GroundHeights, height_binding};
use mln_engine::draw::projection::{TileTransform, tile_projection, zero_to_one_clip_z};
use mln_engine::draw::{
    Composite, Depth, Draw, DrawRange, Draws, EvaluatedLight, IndexBuffer, LayerFrame, Mesh, MeshDesc, RenderPass,
    Resources, VertexAttribute, VertexBinding, VertexBuffer,
};
use mln_engine::layer::binders::PaintBinders as _;
use mln_engine::layer::{
    BucketGeometry, EvaluateCx, EvaluatedLayer, Evaluation, Features, FrameTile, Layer, LayerValues, OnTerrain,
    PrepareCx, QueryRegion, TileCx, TileData, bucket_indexes, bucket_vertices, derived_vertices, paint_vertices,
};
use mln_engine::tile::{OverscaledTileID, UnwrappedTileID};
use mln_gfx::gpu::{Caps, ClipDepthRange, ShaderKey};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::Program;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_style::constants::TILE_SIZE_D;
use mln_style::layers::FillExtrusion;
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionPaintProperties;
use mln_style::types::{AlignmentType, LayerBlendType};

use crate::layers::atlas_binding::AtlasBinding;
use crate::layers::fill_extrusion::bucket::{
    FillExtrusionBinders, FillExtrusionBucket, FillExtrusionLayoutSpec, FillExtrusionLayoutVertex,
    FillExtrusionStaticVertex, FillExtrusionWallVertex, wall_quad_paint,
};
use crate::layers::fill_extrusion::programs;
use crate::layers::fill_extrusion::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use crate::layers::pattern_layout::PatternLayout;
use crate::layers::quads::quads_ranges;
use crate::layers::segments::SegmentBase;

/// The ids of the buffers that the draws bind.
const WALL_VERTICES_ID: usize = programs::FILL_EXTRUSION_PULLED.buffer_id("wallVertices");
const WALL_PAINT_ID: usize = programs::FILL_EXTRUSION_PULLED.buffer_id("wallPaint");
const WALL_HEIGHTS_ID: usize = programs::FILL_EXTRUSION_PULLED.buffer_id("wallHeights");

/// The slots of the programs' textures that the draws bind.
const IMAGE_SLOT: usize = programs::FILL_EXTRUSION_PATTERN.texture_slot("image");

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// How far behind its surfaces a draw into depth only writes their depth, in
/// the clip space depth that runs from 0 to 1: 16 steps of a 24-bit depth
/// buffer.
///
/// Every tile whose buffer reaches a wall draws it, each with its own matrix,
/// which rounds the wall's depth its own way in the last bits, and places a
/// pattern on it its own way. Were the depth pass exact, a draw in color would
/// pass the depth test only where its own tile's copy rounded nearest, and a
/// patterned wall would mix the tiles' patterns in bands that depend on the
/// GPU. Set back, every copy passes, and the first to draw a pixel, which the
/// layer's stencil then keeps, draws all of it. Surfaces closer than this
/// draw in the order of their draws rather than of their depth.
const DEPTH_PASS_SETBACK: f64 = 1.0 / (1 << 20) as f64;

/// A draw the layer keeps of a tile: of its roofs or its walls, into depth
/// only or in color.
struct ExtrusionDraw {
    mesh: Mesh,
    /// 0 for a draw into depth only, which draws before those in color.
    priority: i64,
    /// Whether it draws the walls rather than the roofs.
    walls: bool,
    /// Whether it tests the stencil value the layer's 3D draws share, which
    /// keeps the draws in color from blending a pixel twice.
    clip: bool,
    /// Gives the mesh its pattern's atlas before each frame draws it.
    atlases: Option<AtlasBinding>,
}

/// What the layer keeps of a tile: the draws of its roofs and walls, as they
/// were built for the tile's bucket.
pub(crate) struct ExtrusionTile {
    draws: Vec<ExtrusionDraw>,
    /// Where the pulled walls' shader finds a vertex's paint values
    /// (`FillExtrusionDrawableUBO::paint_layout`).
    paint_layout: u32,
    /// Over terrain, the height of the ground under each bucket vertex's
    /// polygon, and for walls that draw as quads of their own, each four
    /// times over, as the quads' vertices.
    heights: GroundHeights,
    quad_heights: GroundHeights,
}

impl ExtrusionTile {
    /// Over `ground`, has the draws read the height of the ground under the
    /// centroid of each vertex's polygon in `tile`, from `centroids`
    /// ([`FillExtrusionBucket::centroids`]), drawing their walls as `walls`
    /// says: those of the ground they were of while it stays the frame's,
    /// else written again, through `res`.
    fn stand(
        &mut self,
        res: &mut Resources<'_>,
        ground: &FrameGround,
        tile: UnwrappedTileID,
        centroids: &[([i16; 2], u32)],
        walls: Walls,
    ) {
        if self.heights.are_of(ground) {
            return;
        }
        let tile_ground = ground.tile(tile);
        let heights: Vec<f32> = centroids
            .iter()
            .flat_map(|&([x, y], count)| {
                let height = tile_ground.height(glam::Vec2::new(f32::from(x), f32::from(y)));
                std::iter::repeat_n(height, count as usize)
            })
            .collect();
        let quad_heights = (walls == Walls::Quads)
            .then(|| heights.iter().flat_map(|&height| [height; 4]).collect::<Vec<f32>>())
            .and_then(|quad_heights| self.quad_heights.write(res, ground, &quad_heights));
        let heights = self.heights.write(res, ground, &heights);
        // Draws read heights that were written over still.
        for draw in &mut self.draws {
            match (draw.walls, walls) {
                (true, Walls::Pulled) => {
                    if let Some(heights) = &heights {
                        draw.mesh.read_as_storage(res, WALL_HEIGHTS_ID, Some(heights.clone()));
                    }
                }
                (true, Walls::Quads) => {
                    if let Some(quad_heights) = &quad_heights {
                        draw.mesh.set_paint(res, &height_binding(quad_heights, "ground_height"));
                    }
                }
                // The roofs read them per vertex, the instanced walls per
                // instance.
                _ => {
                    if let Some(heights) = &heights {
                        draw.mesh.set_paint(res, &height_binding(heights, "ground_height"));
                    }
                }
            }
        }
    }
}

/// The geometry of draws: the vertices and instances they read, and the
/// ranges of indexes they draw.
struct SharedGeometry<'a> {
    vertices: &'a [VertexBinding],
    instances: Option<&'a [VertexBinding]>,
    vertex_count: usize,
    mode: DrawMode,
    indexes: &'a IndexBuffer,
    ranges: &'a [DrawRange],
}

/// Whether the walls draw as instances of the static quad, rather than as
/// quads of their own, which a device that rasterizes on the CPU draws.
pub(crate) fn instanced_walls(caps: &Caps) -> bool {
    !caps.cpu_rasterizer
}

/// How a layer's walls draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walls {
    /// Instances of the static quad, one per bucket vertex.
    Instanced,
    /// Quads of their own in a vertex buffer.
    Quads,
    /// Quads without attributes, whose shader reads the bucket's vertices
    /// and paint values by vertex index.
    Pulled,
}

impl Walls {
    /// How the walls of a layer draw on `res`'s device.
    fn of(res: &Resources<'_>, has_pattern: bool) -> Walls {
        if !instanced_walls(res.caps()) {
            Walls::Quads
        } else if res.caps().binds_vertex_buffers && !has_pattern {
            Walls::Pulled
        } else {
            Walls::Instanced
        }
    }
}

/// Where the shader of the pulled walls finds a vertex's paint values in
/// the vertices that `paint`, the paint attributes, reads
/// (`FillExtrusionDrawableUBO::paint_layout`).
pub(crate) fn paint_layout(paint: Option<&VertexBinding>) -> u32 {
    let (base, height, color) = ("base", "height", "color");
    let attribute = |name: &str| paint.and_then(|paint| paint.attribute(name));
    let words = |bytes: u32| (bytes / 4) & 0xff;
    let offset = |name: &str| attribute(name).map_or(0, |attribute| words(attribute.offset));
    let reads = [base, height, color].into_iter().any(|name| attribute(name).is_some());
    let stride = paint.filter(|_| reads).map_or(0, |paint| words(paint.stride));
    stride | offset(base) << 8 | offset(height) << 16 | offset(color) << 24
}

/// `paint`, the paint attributes of the bucket's vertices, read from
/// `quad_paint` (`wall_quad_paint`), which has the same layout, where
/// there is one: the binders apply their attributes only to a vector of the
/// bucket's vertices.
fn read_quad_paint(paint: &VertexBinding, quad_paint: Option<&VertexBuffer>) -> VertexBinding {
    match quad_paint {
        Some(quad_paint) => paint.clone().with_buffer(quad_paint),
        None => paint.clone(),
    }
}

/// The attribute `name`, read as `format` at `offset`.
const fn attribute(name: &'static str, format: AttributeDataType, offset: usize) -> VertexAttribute {
    VertexAttribute { name, format, offset: offset as u32 }
}

/// Reads each wall vertex's corner and its wall's outline vertex from
/// `quads` (`wall_quad_vertices`), and the next outline vertex from the same
/// buffer four vertices further on.
fn read_wall_quads(quads: &VertexBuffer) -> [VertexBinding; 2] {
    const CORNER: usize = offset_of!(FillExtrusionWallVertex, corner);
    const POS: usize = offset_of!(FillExtrusionWallVertex, vertex);
    const DECIMALS_ED: usize = POS + offset_of!(FillExtrusionLayoutVertex, a2);
    const WALL: &[VertexAttribute] = &[
        attribute("pos", AttributeDataType::Short2, CORNER),
        attribute("outline_pos", AttributeDataType::Short2, POS),
        attribute("decimals_ed", AttributeDataType::UShort2, DECIMALS_ED),
    ];
    const NEXT: &[VertexAttribute] = &[
        attribute("next_outline_pos", AttributeDataType::Short2, POS),
        attribute("next_decimals_ed", AttributeDataType::UShort2, DECIMALS_ED),
    ];
    let stride = size_of::<FillExtrusionWallVertex>() as u32;
    [VertexBinding::new(quads, stride, WALL), VertexBinding::new(quads, stride, NEXT).starting_at(4)]
}

/// Reads each vertex's corner of the static quad from `corners`, and each
/// instance's vertex and the next one from `vertices`, the bucket's: the
/// bindings of the vertices, then of the instances.
fn read_wall_instances(corners: &VertexBuffer, vertices: &VertexBuffer) -> (VertexBinding, [VertexBinding; 2]) {
    const CORNER: &[VertexAttribute] = &[attribute("pos", AttributeDataType::Short2, 0)];
    const OUTLINE: &[VertexAttribute] = &[
        attribute("outline_pos", AttributeDataType::Short2, 0),
        attribute("decimals_ed", AttributeDataType::UShort2, 4),
    ];
    const NEXT: &[VertexAttribute] = &[
        attribute("next_outline_pos", AttributeDataType::Short2, 0),
        attribute("next_decimals_ed", AttributeDataType::UShort2, 4),
    ];
    let static_stride = size_of::<FillExtrusionStaticVertex>() as u32;
    let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
    // An instance's outline vertex, then the next one.
    (
        VertexBinding::new(corners, static_stride, CORNER),
        [VertexBinding::new(vertices, stride, OUTLINE), VertexBinding::new(vertices, stride, NEXT).starting_at(1)],
    )
}

/// A draw of `geometry` with `shader`, in color with `color_mode` or, without
/// one, into depth only, which binds the textures of `atlases`, those it
/// was made with and again before each frame draws it.
fn extrusion_draw(
    res: &mut Resources<'_>,
    shader: &ShaderKey,
    walls: bool,
    color_mode: Option<ColorMode>,
    clip: bool,
    geometry: &SharedGeometry<'_>,
    atlases: Option<&AtlasBinding>,
) -> ExtrusionDraw {
    let in_color = color_mode.is_some();
    let color_mode = color_mode.unwrap_or_else(ColorMode::disabled);
    let textures = atlases.map(|atlases| atlases.textures(shader)).unwrap_or_default();
    let mesh = res.mesh(MeshDesc {
        shader: shader.clone(),
        color_mode,
        cull_face_mode: CullFaceMode::back_ccw(),
        mode: geometry.mode,
        vertices: geometry.vertices.to_vec(),
        vertex_count: if geometry.ranges.is_empty() { 0 } else { geometry.vertex_count },
        instances: geometry.instances.map(<[VertexBinding]>::to_vec),
        indexes: geometry.indexes.clone(),
        ranges: geometry.ranges.to_vec(),
        textures,
    });
    ExtrusionDraw { mesh, priority: i64::from(in_color), walls, clip, atlases: atlases.cloned() }
}

pub(crate) struct RenderFillExtrusionLayer {
    /// The wall quad (`crate::layers::fill_extrusion::fill_extrusion_vertices`) of the
    /// instanced walls.
    static_data_vertices: Option<VertexBuffer>,
    /// The triangle strip of the wall quad, which the instanced walls draw.
    strip_indices: Option<IndexBuffer>,
    /// The triangles of walls four vertices apart
    /// ([`Resources::quad_indexes`]), which the others draw.
    wall_indices: Option<IndexBuffer>,
}

/// The layer's opacity: as `fill-extrusion-opacity` is the same for every
/// extrusion, both opacities apply to the layer drawn as one surface.
fn opacity(evaluated: &EvaluatedLayer<FillExtrusion>) -> f32 {
    evaluated.paint.fill_extrusion_layer_opacity.uniform() * evaluated.paint.fill_extrusion_opacity.uniform()
}

/// The composite the layer of the values `evaluated`, with or without a
/// pattern, draws into in `frame`: one at its opacity where it is
/// translucent without a pattern and the composite's texture can draw with
/// the frame's depth.
fn composite(
    evaluated: &EvaluatedLayer<FillExtrusion>,
    has_pattern: bool,
    frame: &LayerFrame<'_>,
) -> Option<Composite> {
    let opacity = opacity(evaluated);
    (opacity < 1.0 && !has_pattern && frame.composites_with_frame_depth).then_some(Composite {
        opacity,
        blend: LayerBlendType::Normal,
        frame_depth: true,
    })
}

impl Layer for RenderFillExtrusionLayer {
    type Style = FillExtrusion;
    type Input = Features<PatternLayout<FillExtrusionLayoutSpec>>;
    const CROSSFADES: bool = true;

    fn query_radius(_data: TileData<'_, RenderFillExtrusionLayer>, layer: &EvaluatedLayer<FillExtrusion>) -> f32 {
        let translate = layer.paint.fill_extrusion_translate.uniform();
        (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }
    type Tile = ExtrusionTile;

    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: false };
    const VOLUME: bool = true;

    fn new(_values: &LayerValues<FillExtrusion>) -> Self {
        RenderFillExtrusionLayer { static_data_vertices: None, strip_indices: None, wall_indices: None }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let evaluated = &*cx.evaluated;
        if opacity(evaluated) > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// The draws of a tile's roofs and walls.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, RenderFillExtrusionLayer>,
        _data: TileData<'_, RenderFillExtrusionLayer>,
        _previous: Option<ExtrusionTile>,
    ) -> Option<ExtrusionTile> {
        let render_data = tile.data_for_pass(RENDER_PASS)?;
        let binders = render_data.bucket.paint_property_binders.get(cx.base.style.base().id.as_str())?;
        let has_pattern = !cx.values.unevaluated.fill_extrusion_pattern.is_undefined();
        let walls = Walls::of(&cx.res, has_pattern);
        let (program, wall_program) = match (has_pattern, walls) {
            (false, Walls::Instanced) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_INSTANCED),
            (false, Walls::Quads) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_WALL),
            (false, Walls::Pulled) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_PULLED),
            (true, Walls::Quads) => (&programs::FILL_EXTRUSION_PATTERN, &programs::FILL_EXTRUSION_PATTERN_WALL),
            (true, Walls::Instanced | Walls::Pulled) => {
                (&programs::FILL_EXTRUSION_PATTERN, &programs::FILL_EXTRUSION_PATTERN_INSTANCED)
            }
        };

        let layer = &*cx.values.evaluated;
        let opacity = opacity(layer);
        let composite = composite(layer, has_pattern, cx.frame);

        let res = &mut cx.res;
        let static_vertices = &*self
            .static_data_vertices
            .get_or_insert_with(|| res.vertices(&crate::layers::fill_extrusion::fill_extrusion_vertices()));
        let static_indices = if walls == Walls::Instanced {
            &*self
                .strip_indices
                .get_or_insert_with(|| res.indexes(&crate::layers::fill_extrusion::fill_extrusion_strip_indices()))
        } else {
            &*self.wall_indices.get_or_insert_with(|| res.quad_indexes())
        };
        let builder = TileBuilder {
            program,
            wall_program,
            projection_variant: cx.base.projection_variant,
            walls,
            static_vertices,
            static_indices,
            has_pattern,
            // The layer draws into depth before it draws in color where it
            // is translucent or has a pattern, unless it composites.
            depth_pass: composite.is_none() && (opacity < 1.0 || has_pattern),
        };
        Some(builder.build(res, render_data, binders, &layer.paint))
    }

    /// Pushes the frame's draws of the layer, those of each of its tiles.
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderFillExtrusionLayer>],
        out: &mut Draws,
    ) {
        let has_pattern = !cx.values.unevaluated.fill_extrusion_pattern.is_undefined();
        let walls = Walls::of(&cx.res, has_pattern);
        let layer = &*cx.values.evaluated;
        let opacity = opacity(layer);
        let composite = composite(layer, has_pattern, cx.frame);

        let layer_id = cx.base.style.base().id.as_str();
        let cull_face_mode = cx.frame.cull_back_faces();
        // Over terrain, the extrusions stand on the frame's ground.
        let ground = cx.frame.ground.as_deref();
        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(render_data) = tile.data_for_pass(RENDER_PASS) else {
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(layer_id) else {
                continue;
            };
            let Some(mut entry) = cx.tiles.entry(&tile_id) else {
                continue;
            };

            let changes = entry.take_changes();
            let Some(cached) = entry.value.as_mut() else {
                continue;
            };
            let res = &mut cx.res;
            if changes.paint {
                cached.refresh_paint(res, walls, render_data, binders, &layer.paint);
            }
            if let Some(ground) = ground {
                cached.stand(res, ground, tile_id.to_unwrapped(), &bucket.centroids, walls);
            }
            for draw in &cached.draws {
                draw.mesh.set_cull_face_mode(res, cull_face_mode);
                let frame_draw = Draw {
                    // By priority, then in the order the meshes were made: a
                    // mesh's seq stays far below 2^56.
                    order: (draw.priority << 56) + draw.mesh.seq(),
                    opaque: RENDER_PASS & RenderPass::Opaque != RenderPass::None,
                    depth: Depth::Volume { mask: DepthMaskType::ReadWrite },
                    clip: draw.clip,
                };
                let layout = cached.paint_layout;
                let (drawable, tile_props, projection) =
                    Self::uniforms(layer, tile_id, render_data, binders, draw, layout, res, cx.frame);
                if let Some(atlases) = &draw.atlases {
                    atlases.bind(&draw.mesh, res, cx.frame);
                }
                if has_pattern {
                    out.main().push_at(&draw.mesh, tile_id, &projection, frame_draw, (drawable, tile_props));
                } else {
                    out.main().push_at(&draw.mesh, tile_id, &projection, frame_draw, drawable);
                }
            }
        }

        let (light, state) = (cx.frame.light, cx.frame.view);
        // The draws that composite draw opaque: the composite has the
        // layer's opacity.
        let opacity = if composite.is_some() { 1.0 } else { opacity };
        out.main().composite(composite).block(&Self::props(layer, light, state, opacity));
    }

    // query
    fn hit(
        &self,
        values: &LayerValues<FillExtrusion>,
        region: &QueryRegion<'_>,
        _feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        _feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        let evaluated = &values.evaluated.paint;
        let translated_query_geometry = mln_engine::query::translate_query_geometry(
            region.geometry,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            region.view.angle() as f32,
            region.pixels_to_tile_units,
        );

        mln_engine::geo::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(region.geometry),
            geometry,
        )
    }
}

/// What building the draws of a tile reads of the layer.
struct TileBuilder<'a> {
    /// The programs of the roofs and of the walls.
    program: &'static Program,
    wall_program: &'static Program,
    projection_variant: mln_gfx::projection_variant::ProjectionVariant,
    /// How the walls draw.
    walls: Walls,
    /// The static quad, and the indexes the walls draw.
    static_vertices: &'a VertexBuffer,
    static_indices: &'a IndexBuffer,
    has_pattern: bool,
    /// Whether the layer draws into depth before it draws in color.
    depth_pass: bool,
}

impl TileBuilder<'_> {
    /// The draws of `tile`'s `bucket`, laid out with the layer's `evaluated`
    /// values: of its roofs, then of its walls, each into depth, where the
    /// layer draws it, then in color.
    fn build(
        &self,
        res: &mut Resources<'_>,
        tile_bucket: TileData<'_, RenderFillExtrusionLayer>,
        binders: &FillExtrusionBinders,
        evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    ) -> ExtrusionTile {
        let (bucket, geometry) = (tile_bucket.bucket, tile_bucket.geometry());
        let (Some(vertices), Some(triangles)) =
            (bucket_vertices(geometry, &bucket.vertices), bucket_indexes(geometry, &bucket.triangles))
        else {
            return ExtrusionTile {
                draws: Vec::new(),
                paint_layout: 0,
                heights: GroundHeights::default(),
                quad_heights: GroundHeights::default(),
            };
        };
        let vertex_count = bucket.vertices.len();
        // A wall from each vertex to the next, but from the last, which
        // ends a ring.
        let walls = vertex_count.saturating_sub(1);
        let buffer = &binders.interleaved_vertex_buffer;
        let depends_on_feature_state = binders.depends_on_feature_state();
        // The walls' own quads, which the bucket's upload made of its
        // vertices and paint values before those moved to the vectors the
        // roofs draw.
        let quads = (self.walls == Walls::Quads).then(|| {
            let vertices = derived_vertices(geometry, &bucket.vertices);
            (vertices, wall_paint(res, geometry, binders, walls))
        });

        // The roofs read the paint values per vertex, the instanced walls per
        // instance and the pulled walls by vertex index, from the same
        // buffer.
        let paint = paint_vertices(res, geometry, buffer, depends_on_feature_state);
        let paint = paint.as_ref();
        let paint_attributes = binders.paint(evaluated, paint);
        // The roofs' and the walls' programs read the same properties from
        // uniforms.
        let uniforms = paint_attributes.uniforms;
        let paint_binding = paint_attributes.vertices;
        let mut wall_vertices: Vec<VertexBinding> = Vec::new();
        let mut paint_layout = 0;
        let (wall_instances, wall_vertex_count, wall_ranges) = if let Some((quad_vertices, quad_paint)) = &quads {
            wall_vertices.extend(paint_binding.iter().map(|paint| read_quad_paint(paint, quad_paint.as_ref())));
            if let Some(quad_vertices) = quad_vertices {
                wall_vertices.extend(read_wall_quads(quad_vertices));
            }
            (None, 4 * walls, quads_ranges(walls))
        } else if self.walls == Walls::Pulled {
            // The walls have no attributes: these say which properties the
            // shader reads from the paint values, and where.
            paint_layout = self::paint_layout(paint_binding.as_ref());
            (None, 4 * walls, quads_ranges(walls))
        } else {
            let (corners, outline) = read_wall_instances(self.static_vertices, &vertices);
            wall_vertices.push(corners);
            let instances: Vec<VertexBinding> = paint_binding.iter().cloned().chain(outline).collect();
            let ranges = vec![crate::layers::fill_extrusion::fill_extrusion_segment(walls).range()];
            (Some(instances), self.static_vertices.count(), ranges)
        };

        let shader = res.shader_key(self.program, uniforms, self.projection_variant);
        let wall_shader = res.shader_key(self.wall_program, uniforms, self.projection_variant);
        let color = Some(ColorMode::alpha_blended());

        let roof_vertices: Vec<VertexBinding> =
            paint_binding.iter().cloned().chain([VertexBinding::of::<FillExtrusionLayoutVertex>(&vertices)]).collect();

        let atlases = self.has_pattern.then(|| tile_bucket.atlas_textures()).map(|atlases| {
            AtlasBinding::new(
                atlases,
                None,
                Some(IMAGE_SLOT),
                /*is_text=*/ false,
                false,
                AlignmentType::Auto,
                false,
                false,
            )
        });
        let mut draws = Vec::new();
        // The draws in color clip where the layer draws into depth first.
        let clip = self.depth_pass;
        let roof_ranges: Vec<DrawRange> = bucket.triangle_segments.iter().map(SegmentBase::range).collect();
        let roofs = SharedGeometry {
            vertices: &roof_vertices,
            instances: None,
            vertex_count,
            mode: DrawMode::Triangles(),
            indexes: &triangles,
            ranges: &roof_ranges,
        };
        if !bucket.triangles.is_empty() && vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(res, &shader, false, None, false, &roofs, atlases.as_ref()));
            }
            draws.push(extrusion_draw(res, &shader, false, color, clip, &roofs, atlases.as_ref()));
        }
        // A tile with one vertex has no wall.
        let walls_geometry = SharedGeometry {
            vertices: &wall_vertices,
            instances: wall_instances.as_deref(),
            vertex_count: wall_vertex_count,
            mode: if self.walls == Walls::Instanced { DrawMode::TriangleStrip() } else { DrawMode::Triangles() },
            indexes: self.static_indices,
            ranges: &wall_ranges,
        };
        if walls > 0 && wall_vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(res, &wall_shader, true, None, false, &walls_geometry, atlases.as_ref()));
            }
            draws.push(extrusion_draw(res, &wall_shader, true, color, clip, &walls_geometry, atlases.as_ref()));
        }
        if self.walls == Walls::Pulled {
            for draw in draws.iter_mut().filter(|draw| draw.walls) {
                draw.mesh.read_as_storage(res, WALL_VERTICES_ID, Some(vertices.clone()));
                draw.mesh.read_as_storage(res, WALL_PAINT_ID, paint.cloned());
            }
        }

        ExtrusionTile { draws, paint_layout, heights: GroundHeights::default(), quad_heights: GroundHeights::default() }
    }
}

/// The paint values of the walls' own quads, which the bucket's upload made
/// of its paint values before those moved to the vectors the roofs draw,
/// unless feature state rewrites them.
fn wall_paint(
    res: &mut Resources<'_>,
    geometry: &BucketGeometry,
    binders: &FillExtrusionBinders,
    walls: usize,
) -> Option<VertexBuffer> {
    let buffer = &binders.interleaved_vertex_buffer;
    if binders.depends_on_feature_state() {
        let paint = wall_quad_paint(&buffer.shared_vertex_vector.data(), buffer.stride, walls);
        Some(res.vertices(&paint))
    } else {
        derived_vertices(geometry, &buffer.shared_vertex_vector)
    }
}

impl ExtrusionTile {
    /// Points the tile's draws, whose walls draw as `walls`, at the paint
    /// values of its bucket's `binders` that feature state rewrote.
    fn refresh_paint(
        &mut self,
        res: &mut Resources<'_>,
        walls: Walls,
        tile_bucket: TileData<'_, RenderFillExtrusionLayer>,
        binders: &FillExtrusionBinders,
        evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    ) {
        let geometry = tile_bucket.geometry();
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(res, geometry, buffer, binders.depends_on_feature_state());
        let wall_count = tile_bucket.bucket.vertices.len().saturating_sub(1);
        let quad_paint = (walls == Walls::Quads).then(|| wall_paint(res, geometry, binders, wall_count)).flatten();
        let paint_binding = binders.paint(evaluated, paint.as_ref()).vertices;
        for draw in &self.draws {
            if let Some(binding) = &paint_binding {
                let quad_paint = quad_paint.as_ref().filter(|_| draw.walls);
                draw.mesh.set_paint(res, &read_quad_paint(binding, quad_paint));
            }
            if walls == Walls::Pulled && draw.walls {
                self.paint_layout = paint_layout(paint_binding.as_ref());
                draw.mesh.read_as_storage(res, WALL_PAINT_ID, paint.clone());
            }
        }
    }
}

impl RenderFillExtrusionLayer {
    /// The blocks and the projection of `draw`, a draw of `tile`, whose
    /// bucket's binders are `binders` and whose pulled walls find their
    /// paint values by `paint_layout`, with the layer's values `layer`, in
    /// `frame`, whose textures `res` has. The tile's block is read
    /// only with a pattern.
    #[expect(
        clippy::too_many_arguments,
        reason = "the layer, the tile, its bucket and binders, the draw, its paint layout, the resources and the frame"
    )]
    fn uniforms(
        layer: &EvaluatedLayer<FillExtrusion>,
        tile_id: OverscaledTileID,
        tile_bucket: TileData<'_, RenderFillExtrusionLayer>,
        binders: &FillExtrusionBinders,
        draw: &ExtrusionDraw,
        paint_layout: u32,
        res: &Resources<'_>,
        frame: &LayerFrame<'_>,
    ) -> (FillExtrusionDrawableUBO, FillExtrusionTilePropsUBO, ProjectionUBO) {
        // The near-clipped projection keeps the depth buffer's precision for
        // the extrusions, but over terrain they test the frame's depth of the
        // ground, which the surface's projection draws, and the frame reports
        // that projection (`frame_projection::compute`).
        let near_clipped = !frame.terrain;
        let evaluated = &layer.paint;
        let state = frame.view;
        let zoom = state.zoom() as f32;
        let tile_id = tile_id.to_unwrapped();

        let mut projection = tile_projection(
            &tile_id,
            frame,
            TileTransform {
                translate: *evaluated.fill_extrusion_translate.uniform(),
                anchor: *evaluated.fill_extrusion_translate_anchor.uniform(),
                near_clipped,
                ..TileTransform::NONE
            },
        );
        if draw.priority == 0 {
            // z + setback * w: x, y and w, and so the pixels the draw
            // covers, stay those of the draw in color.
            for matrix in [&mut projection.main_matrix, &mut projection.fallback_matrix] {
                matrix.set_row(2, matrix.row(2) + DEPTH_PASS_SETBACK * matrix.row(3));
            }
        }
        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0 (`frame_projection`). The globe projects in its
        // shaders.
        if res.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !state.is_globe() {
            zero_to_one_clip_z(&mut projection.main_matrix);
            zero_to_one_clip_z(&mut projection.fallback_matrix);
        }

        let int_zoom = state.integer_zoom();
        let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
        let zoom_scale = mln_engine::camera::zoom_scale(f64::from(tile_id.canonical.z));
        let nearest_zoom_scale = mln_engine::camera::zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z));
        let tile_size_at_nearest_zoom = (TILE_SIZE_D * nearest_zoom_scale).floor();
        let pixel_x = (tile_size_at_nearest_zoom
            * (f64::from(tile_id.canonical.x) + f64::from(tile_id.wrap) * zoom_scale)) as i32;
        let pixel_y = (tile_size_at_nearest_zoom * f64::from(tile_id.canonical.y)) as i32;
        let num_tiles = 2f64.powi(i32::from(tile_id.canonical.z));
        let height_factor = (-num_tiles / TILE_SIZE_D / 8.0) as f32;

        let texture_size =
            draw.mesh.texture(res, IMAGE_SLOT).map(|texture| res.texture_size(texture)).unwrap_or_default();
        let pattern = evaluated.fill_extrusion_pattern.faded();
        let tlbr = |id: &str| tile_bucket.pattern(id).map_or([0.0; 4], |position| position.tlbr().map(f32::from));

        let pattern_t = binders.fill_extrusion_pattern.interpolation_factor(zoom).0;
        let drawable = FillExtrusionDrawableUBO {
            matrix: projection.fallback_matrix.as_mat4().to_cols_array(),
            pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
            pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
            height_factor,
            tile_ratio,
            base_t: binders.fill_extrusion_base.interpolation_factor(zoom),
            height_t: binders.fill_extrusion_height.interpolation_factor(zoom),
            color_t: binders.fill_extrusion_color.interpolation_factor(zoom),
            pattern_from_t: pattern_t,
            pattern_to_t: pattern_t,
            paint_layout,
        };
        let tile_props = FillExtrusionTilePropsUBO {
            pattern_from: tlbr(pattern.from.id()),
            pattern_to: tlbr(pattern.to.id()),
            texsize: [texture_size.width as f32, texture_size.height as f32],
            pad1: 0.0,
            pad2: 0.0,
        };
        (drawable, tile_props, projection.ubo())
    }

    /// The block of the layer's values that its draws share, which draw at
    /// `opacity`.
    fn props(
        layer: &EvaluatedLayer<FillExtrusion>,
        light: &EvaluatedLight,
        state: &View,
        opacity: f32,
    ) -> FillExtrusionPropsUBO {
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        FillExtrusionPropsUBO {
            color: evaluated
                .fill_extrusion_color
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_color())
                .to_float_array(),
            light_color: FillExtrusionBucket::light_color(light),
            pad1: 0.0,
            light_position: FillExtrusionBucket::light_position(light, state),
            base: evaluated
                .fill_extrusion_base
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_base()),
            height: evaluated
                .fill_extrusion_height
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_height()),
            light_intensity: FillExtrusionBucket::light_intensity(light),
            vertical_gradient: if *evaluated.fill_extrusion_vertical_gradient.uniform() { 1.0 } else { 0.0 },
            opacity,
            fade: crossfade.t,
            from_scale: crossfade.from_scale,
            to_scale: crossfade.to_scale,
            pad2: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;

    use super::*;
    use crate::layers::fill_extrusion::bucket::wall_quad_vertices;

    // A wall's quad is the static quad's corners, each with the wall's
    // vertex, and the next four are the next vertex's. Its paint values are
    // its instance's, in each corner.
    #[test]
    fn wall_quads_are_the_instances() {
        let vertices: Vec<FillExtrusionLayoutVertex> = (0..4)
            .map(|i| FillExtrusionBucket::layout_vertex(Point::new(i as i16, 1), i as u16, i == 3, None))
            .collect();
        let quads = wall_quad_vertices(&vertices);
        assert_eq!(quads.len(), 4 * 4);
        let corners = crate::layers::fill_extrusion::fill_extrusion_vertices();
        for (vertex, quad) in vertices.iter().zip(quads.chunks(4)) {
            for (&corner, wall_vertex) in corners.iter().zip(quad) {
                assert_eq!(*wall_vertex, FillExtrusionWallVertex { corner, vertex: *vertex });
            }
        }

        // Four vertices of three bytes: the last one's starts no wall.
        let paint: Vec<u8> = (0..12).collect();
        let quad_paint = wall_quad_paint(&paint, 3, 3);
        let expected: Vec<u8> = paint[..9].chunks(3).flat_map(|values| values.repeat(4)).collect();
        assert_eq!(quad_paint, expected);
        assert_eq!(wall_quad_paint(&[], 0, 3).len(), 0);
    }

    // The ranges draw every wall, each from the start of the quad indexes,
    // which address no more vertices than 16 bits do.
    #[test]
    fn wall_ranges_draw_every_wall() {
        let most = crate::layers::segments::MAX_SEGMENT_VERTICES / 4;
        assert_eq!(quads_ranges(0).len(), 0);
        let lengths: Vec<_> = quads_ranges(most + 2)
            .iter()
            .map(|r| (r.base_vertex, r.first_index, r.index_count, r.instance_count))
            .collect();
        let most = most as u32;
        assert_eq!(lengths, [(0, 0, 6 * most, 1), (4 * most as i32, 0, 12, 1)]);
    }
}
