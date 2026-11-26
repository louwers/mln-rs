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
use std::sync::Arc;

use mln_gfx::gpu::{ClipDepthRange, Gpu, ShaderKey};
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::Program;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_map_types::View;
use mln_style::constants::TILE_SIZE_D;
use mln_style::error::Error;
use mln_style::layer::LayerStyle;
use mln_style::layers::FillExtrusion;
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionPaintProperties;
use mln_style::types::{AlignmentType, LayerBlendType};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use super::cast_matrix;
use crate::bucket::Bucket;
use crate::bucket_geometry::{
    BucketGeometry, bucket_indexes, bucket_vertices, copied_vertices, derived_vertices, paint_vertices,
};
use crate::buckets::fill_extrusion_bucket::{
    FillExtrusionBinders, FillExtrusionBucket, FillExtrusionLayoutSpec, FillExtrusionLayoutVertex,
    FillExtrusionStaticVertex, FillExtrusionWallVertex, wall_quad_paint,
};
use crate::draw::{DrawGeometry, DrawObjects, copy_shared_attributes};
use crate::frame_projection::zero_to_one_clip_z;
use crate::geometry::feature_index::QueryRegion;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::indexes::{Indexes, SharedIndexes};
use crate::gfx::segment::SegmentBase;
use crate::gfx::texture2d::Texture2D;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_composite::Composite;
use crate::layer_drawing::{
    DrawDepth, DrawList, DrawObjectsMut, DrawPriority, FrameDraw, TileDraws, TileObjects, UniformWriter,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layers::fill_extrusion::programs;
use crate::layers::fill_extrusion::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{PatternLayout, uses_pattern};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerBucket, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_light::EvaluatedLight;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::terrain::{FrameGround, GroundHeights, height_attribute};
use crate::tile_matrix::{projection_data, to_projection_ubo};

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::FILL_EXTRUSION.buffer_id("drawableVector");
const TILE_PROPS_ID: usize = programs::FILL_EXTRUSION_PATTERN.buffer_id("tilePropsVector");
const PROPS_ID: usize = programs::FILL_EXTRUSION.buffer_id("props");
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
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    /// 0 for a draw into depth only, which draws before those in color.
    priority: DrawPriority,
    /// Whether it draws the walls rather than the roofs.
    walls: bool,
    /// Whether it tests the stencil value the layer's 3D draws share, which
    /// keeps the draws in color from blending a pixel twice.
    clip: bool,
}

/// What the layer keeps of a tile: the draws of its roofs and walls, as they
/// were built for the tile's bucket.
pub(crate) struct ExtrusionTile {
    draws: Vec<ExtrusionDraw>,
    /// Where the pulled walls' shader finds a vertex's paint values
    /// (`FillExtrusionDrawableUBO::paint_layout`).
    paint_layout: u32,
    /// Gives the pattern draws the texture of the tile's icon atlas.
    atlases: Option<AtlasBinding>,
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
    /// else written again, through `context`.
    fn stand(
        &mut self,
        context: &mut Context,
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
            .and_then(|quad_heights| self.quad_heights.write(context, ground, &quad_heights));
        let heights = self.heights.write(context, ground, &heights);
        // Draws read heights that were written over still.
        for draw in &mut self.draws {
            match (draw.walls, walls) {
                (true, Walls::Pulled) => {
                    if let Some(heights) = &heights {
                        draw.objects.read_as_storage(WALL_HEIGHTS_ID, Some(heights.clone()));
                    }
                }
                (true, Walls::Quads) => {
                    if let Some(quad_heights) = &quad_heights {
                        let attribute = height_attribute(quad_heights);
                        draw.objects.paint_attributes_mut().set_shared("ground_height", attribute);
                    }
                }
                // The roofs read them per vertex, the instanced walls per
                // instance.
                _ => {
                    if let Some(heights) = &heights {
                        let attribute = height_attribute(heights);
                        draw.objects.paint_attributes_mut().set_shared("ground_height", attribute);
                    }
                }
            }
        }
    }
}

impl TileObjects for ExtrusionTile {
    /// The index of the draw among the tile's.
    type Part = usize;

    fn objects(&mut self, index: usize) -> Option<DrawObjectsMut<'_>> {
        Some(DrawObjectsMut { objects: &mut self.draws.get_mut(index)?.objects, atlases: self.atlases.as_ref() })
    }

    fn drop_bindings(&mut self) {
        for draw in &mut self.draws {
            draw.objects.drop_bindings();
        }
    }
}

/// A draw's entries in the layer's storage buffers.
struct ExtrusionUniforms {
    draw_ubo: FillExtrusionDrawableUBO,
    tile_props: FillExtrusionTilePropsUBO,
    projection: ProjectionUBO,
}

/// The geometry of draws: vertices in their attributes, which each draw
/// reads a copy of, and the segments of indexes they draw.
struct SharedGeometry<'a> {
    vertices: &'a VertexAttributeArray,
    instances: Option<&'a VertexAttributeArray>,
    vertex_count: usize,
    mode: DrawMode,
    indexes: &'a SharedIndexes,
    segments: &'a [SegmentBase],
}

/// Whether the walls draw as instances of the static quad, rather than as
/// quads of their own, which a device that rasterizes on the CPU draws.
pub(crate) fn instanced_walls(context: &Context) -> bool {
    !context.caps().cpu_rasterizer
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
    /// How the walls of a layer draw on `context`'s device.
    fn of(context: &Context, has_pattern: bool) -> Walls {
        if !instanced_walls(context) {
            Walls::Quads
        } else if context.caps().binds_vertex_buffers && !has_pattern {
            Walls::Pulled
        } else {
            Walls::Instanced
        }
    }
}

/// Where the shader of the pulled walls finds a vertex's paint values in
/// the vertices that `attrs`, the paint attributes, read
/// (`FillExtrusionDrawableUBO::paint_layout`).
pub(crate) fn paint_layout(attrs: &VertexAttributeArray) -> u32 {
    let (base, height, color) = ("base", "height", "color");
    let shared = |name: &str| attrs.get(name).and_then(|attribute| attribute.shared());
    let words = |bytes: u32| (bytes / 4) & 0xff;
    let offset = |name: &str| shared(name).map_or(0, |attribute| words(attribute.offset));
    let stride = [base, height, color].into_iter().find_map(shared).map_or(0, |attribute| words(attribute.stride));
    stride | offset(base) << 8 | offset(height) << 16 | offset(color) << 24
}

/// Points each attribute of `attrs` that reads `paint` at `quad_paint`
/// (`wall_quad_paint`), which has the same layout. The binders apply their
/// attributes only to a vector of the bucket's vertices.
fn read_quad_paint(attrs: &mut VertexAttributeArray, paint: &SharedVertices, quad_paint: &SharedVertices) {
    for shared in attrs.shared_mut().filter(|shared| shared.vertices == *paint) {
        shared.vertices = quad_paint.clone();
    }
}

/// Reads each wall vertex's corner and its wall's outline vertex from
/// `quads` (`wall_quad_vertices`), and the next outline vertex from the same
/// buffer four vertices further on.
fn read_wall_quads(attrs: &mut VertexAttributeArray, quads: &SharedVertices) {
    let stride = size_of::<FillExtrusionWallVertex>() as u32;
    let corner = offset_of!(FillExtrusionWallVertex, corner) as u32;
    let pos = offset_of!(FillExtrusionWallVertex, vertex) as u32;
    let decimals_ed = pos + offset_of!(FillExtrusionLayoutVertex, a2) as u32;
    for (name, offset, vertex_offset, type_) in [
        ("pos", corner, 0, AttributeDataType::Short2),
        ("outline_pos", pos, 0, AttributeDataType::Short2),
        ("decimals_ed", decimals_ed, 0, AttributeDataType::UShort2),
        ("next_outline_pos", pos, 4, AttributeDataType::Short2),
        ("next_decimals_ed", decimals_ed, 4, AttributeDataType::UShort2),
    ] {
        attrs.set_shared(name, SharedAttribute::new(quads.clone(), type_, offset, stride).starting_at(vertex_offset));
    }
}

/// Reads each vertex's corner of the static quad from `corners`, and each
/// instance's vertex and the next one from `vertices`, the bucket's.
fn read_wall_instances(
    attrs: &mut VertexAttributeArray,
    instance_attrs: &mut VertexAttributeArray,
    corners: &SharedVertices,
    vertices: &SharedVertices,
) {
    let static_stride = size_of::<FillExtrusionStaticVertex>() as u32;
    attrs.set_shared("pos", SharedAttribute::new(corners.clone(), AttributeDataType::Short2, 0, static_stride));
    let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
    // An instance's outline vertex, then the next one.
    for (vertex_offset, pos, decimals_ed) in
        [(0, "outline_pos", "decimals_ed"), (1, "next_outline_pos", "next_decimals_ed")]
    {
        instance_attrs.set_shared(
            pos,
            SharedAttribute::new(vertices.clone(), AttributeDataType::Short2, 0, stride).starting_at(vertex_offset),
        );
        instance_attrs.set_shared(
            decimals_ed,
            SharedAttribute::new(vertices.clone(), AttributeDataType::UShort2, 4, stride).starting_at(vertex_offset),
        );
    }
}

/// A draw of `geometry` with `shader`, in color with `color_mode` or, without
/// one, into depth only, which binds the textures of `atlases`.
fn extrusion_draw(
    shader: &ShaderKey,
    walls: bool,
    color_mode: Option<ColorMode>,
    clip: bool,
    geometry: &SharedGeometry<'_>,
    atlases: Option<&AtlasBinding>,
) -> ExtrusionDraw {
    let draw_geometry = DrawGeometry {
        vertices: copy_shared_attributes(geometry.vertices),
        vertex_count: if geometry.segments.is_empty() { 0 } else { geometry.vertex_count },
        mode: geometry.mode,
        indexes: geometry.indexes.clone().into(),
        segments: geometry.segments.iter().map(SegmentBase::copy).collect(),
        instances: geometry.instances.map(copy_shared_attributes),
    };
    let in_color = color_mode.is_some();
    let color_mode = color_mode.unwrap_or_else(ColorMode::disabled);
    let textures = atlases.map(|atlases| atlases.textures(shader)).unwrap_or_default();
    let objects = DrawObjects::new(shader.clone(), color_mode, CullFaceMode::back_ccw(), draw_geometry, textures);
    ExtrusionDraw { id: SimpleIdentity::new(), objects, priority: DrawPriority::from(in_color), walls, clip }
}

impl LayerType for FillExtrusion {
    type Layout = PatternLayout<FillExtrusionLayoutSpec>;
    type Bucket = FillExtrusionBucket;
    type TileBucket<'a> = LayerBucket<'a, FillExtrusion>;
    const CROSSFADES: bool = true;

    /// Whether a layer of the group has a pattern.
    fn layout_can_need_dependencies(group: &[Arc<EvaluatedLayer<FillExtrusion>>]) -> bool {
        uses_pattern::<FillExtrusionLayoutSpec>(group)
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<FillExtrusion>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<PatternLayout<FillExtrusionLayoutSpec>>, Error> {
        PatternLayout::new(parameters, group, source_layer).map(Some)
    }

    fn query_radius(_bucket: &FillExtrusionBucket, layer: &EvaluatedLayer<FillExtrusion>) -> f32 {
        let translate = layer.paint.fill_extrusion_translate.uniform();
        (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }

    fn render_layer(style: Arc<LayerStyle<FillExtrusion>>) -> Box<dyn RenderLayer> {
        Box::new(RenderFillExtrusionLayer::new(style))
    }
}

pub(crate) struct RenderFillExtrusionLayer {
    base: RenderLayerBase,
    values: LayerValues<FillExtrusion>,
    drawing: TileDraws<ExtrusionTile>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<ExtrusionUniforms, usize>,

    /// The wall quad (`RenderStaticData::fill_extrusion_vertices`) of the
    /// instanced walls.
    static_data_vertices: Option<SharedVertices>,
    /// The triangle strip of the wall quad, which the instanced walls draw.
    strip_indices: Option<SharedIndexes>,
    /// The triangles of walls four vertices apart
    /// (`RenderStaticData::wall_indexes`), which the others draw.
    wall_indices: Option<SharedIndexes>,
}

impl RenderFillExtrusionLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<FillExtrusion>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderFillExtrusionLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            static_data_vertices: None,
            strip_indices: None,
            wall_indices: None,
        }
    }
}

/// The layer's opacity: as `fill-extrusion-opacity` is the same for every
/// extrusion, both opacities apply to the layer drawn as one surface.
fn opacity(evaluated: &EvaluatedLayer<FillExtrusion>) -> f32 {
    evaluated.paint.fill_extrusion_layer_opacity.uniform() * evaluated.paint.fill_extrusion_opacity.uniform()
}

impl TypedLayer for RenderFillExtrusionLayer {
    type Type = FillExtrusion;
    type Drawing = TileDraws<ExtrusionTile>;

    const STANDS: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<FillExtrusion>) -> RenderPass {
        if opacity(evaluated) > 0.0 { RenderPass::Translucent } else { RenderPass::None }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, FillExtrusion>],
    ) {
        let has_pattern = !self.values.unevaluated.fill_extrusion_pattern.is_undefined();
        let walls = Walls::of(paint_parameters.context, has_pattern);
        let (program, wall_program) = match (has_pattern, walls) {
            (false, Walls::Instanced) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_INSTANCED),
            (false, Walls::Quads) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_WALL),
            (false, Walls::Pulled) => (&programs::FILL_EXTRUSION, &programs::FILL_EXTRUSION_PULLED),
            (true, Walls::Quads) => (&programs::FILL_EXTRUSION_PATTERN, &programs::FILL_EXTRUSION_PATTERN_WALL),
            (true, Walls::Instanced | Walls::Pulled) => {
                (&programs::FILL_EXTRUSION_PATTERN, &programs::FILL_EXTRUSION_PATTERN_INSTANCED)
            }
        };

        let opacity = opacity(&self.values.evaluated);
        let composite = (opacity < 1.0 && !has_pattern && paint_parameters.composites_with_frame_depth)
            .then_some(Composite { opacity, blend: LayerBlendType::Normal, frame_depth: true });

        let context = &mut *paint_parameters.context;
        let static_vertices = &*self
            .static_data_vertices
            .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::fill_extrusion_vertices()));
        let static_indices = if walls == Walls::Instanced {
            &*self.strip_indices.get_or_insert_with(|| {
                context.add_indexes(Indexes::new(RenderStaticData::fill_extrusion_strip_indices()))
            })
        } else {
            let static_data = paint_parameters.static_data;
            &*self.wall_indices.get_or_insert_with(|| static_data.wall_indexes(context))
        };
        let builder = TileBuilder {
            program,
            wall_program,
            projection_variant: self.base.projection_variant,
            walls,
            static_vertices,
            static_indices,
            has_pattern,
            // The layer draws into depth before it draws in color where it
            // is translucent or has a pattern, unless it composites.
            depth_pass: composite.is_none() && (opacity < 1.0 || has_pattern),
        };

        let layer = &*self.values.evaluated;
        let cull_face_mode = paint_parameters.cull_back_faces();
        // Over terrain, the extrusions stand on the frame's ground.
        let ground = paint_parameters.ground.clone();
        for tile in render_tiles {
            let tile_id = tile.id();
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let render_data = tile.bucket_for_pass(RENDER_PASS);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;
            // Draws built from another bucket go.
            entry.set_bucket(bucket.base.bucket_id);

            let Some(binders) = bucket.paint_property_binders.get(self.base.style.base().id.as_str()) else {
                continue;
            };

            let changes = entry.take_changes();
            let context = &mut *paint_parameters.context;
            let cached = match entry.value {
                Some(cached) => {
                    if changes.paint {
                        builder.refresh_paint(context, cached, render_data, binders, &layer.paint);
                    }
                    cached
                }
                None => entry.value.insert(builder.build(context, render_data, binders, &layer.paint)),
            };
            if let Some(ground) = ground.as_deref() {
                cached.stand(context, ground, tile_id.to_unwrapped(), &bucket.centroids, walls);
            }
            for (index, draw) in cached.draws.iter_mut().enumerate() {
                draw.objects.set_cull_face_mode(cull_face_mode);
                let frame_draw = FrameDraw {
                    priority: draw.priority,
                    pass: RENDER_PASS,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    clip: draw.clip,
                    ..FrameDraw::new(Some(tile_id), draw.id)
                };
                let layout = cached.paint_layout;
                let uniforms = Self::uniforms(layer, tile_id, render_data, binders, draw, layout, paint_parameters);
                self.draw_list.push_part(frame_draw, index, uniforms);
            }
        }

        let frame = self.drawing.frame_mut();
        frame.set_composite(composite);
        frame.set_stencil_tiles(render_tiles.iter().map(|tile| tile.tile.id));
        let (light, state) = (paint_parameters.light, paint_parameters.view);
        // The draws that composite draw opaque: the composite has the
        // layer's opacity.
        let opacity = if composite.is_some() { 1.0 } else { opacity };
        self.drawing.finish(&mut self.draw_list, gpu, paint_parameters.context, |writer| {
            Self::write_uniforms(layer, writer, light, state, opacity);
        });
    }
}

impl RenderLayer for RenderFillExtrusionLayer {
    fn is_3d(&self) -> bool {
        true
    }

    // query
    fn query_intersects_feature(
        &self,
        region: &QueryRegion<'_>,
        _feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        _feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        let evaluated = &self.values.evaluated.paint;
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            region.geometry,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            region.view.angle() as f32,
            region.pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(region.geometry),
            geometry,
        )
    }
}

/// What building the draws of an update's tiles reads of the layer.
struct TileBuilder<'a> {
    /// The programs of the roofs and of the walls.
    program: &'static Program,
    wall_program: &'static Program,
    projection_variant: mln_gfx::projection_variant::ProjectionVariant,
    /// How the walls draw.
    walls: Walls,
    /// The static quad, and the indexes the walls draw.
    static_vertices: &'a SharedVertices,
    static_indices: &'a SharedIndexes,
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
        context: &mut Context,
        tile_bucket: LayerBucket<'_, FillExtrusion>,
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
                atlases: None,
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
            (vertices, wall_paint(context, geometry, binders, walls))
        });

        // The roofs read the paint values per vertex, the instanced walls per
        // instance and the pulled walls by vertex index, from the same
        // buffer.
        let paint = paint_vertices(context, geometry, buffer, depends_on_feature_state);
        let paint = paint.as_ref();
        let mut uniforms = Uniforms::default();
        let mut wall_uniforms = Uniforms::default();
        let mut vertex_attrs = VertexAttributeArray::new();
        binders.bind(evaluated, paint, &mut vertex_attrs, &mut uniforms);
        let mut wall_attrs = VertexAttributeArray::new();
        let mut paint_layout = 0;
        let (wall_instance_attrs, wall_vertex_count, wall_segments) = if let Some((quad_vertices, quad_paint)) = &quads
        {
            binders.bind(evaluated, paint, &mut wall_attrs, &mut wall_uniforms);
            if let (Some(paint), Some(quad_paint)) = (paint, quad_paint) {
                read_quad_paint(&mut wall_attrs, paint, quad_paint);
            }
            if let Some(quad_vertices) = quad_vertices {
                read_wall_quads(&mut wall_attrs, quad_vertices);
            }
            (None, 4 * walls, RenderStaticData::fill_extrusion_wall_segments(walls))
        } else if self.walls == Walls::Pulled {
            // The walls have no attributes: these say which properties the
            // shader reads from the paint values, and where.
            let mut paint_attrs = VertexAttributeArray::new();
            binders.bind(evaluated, paint, &mut paint_attrs, &mut wall_uniforms);
            paint_layout = self::paint_layout(&paint_attrs);
            (None, 4 * walls, RenderStaticData::fill_extrusion_wall_segments(walls))
        } else {
            let mut instance_attrs = VertexAttributeArray::new();
            binders.bind(evaluated, paint, &mut instance_attrs, &mut wall_uniforms);
            read_wall_instances(&mut wall_attrs, &mut instance_attrs, self.static_vertices, &vertices);
            let segments = vec![RenderStaticData::fill_extrusion_segment(walls)];
            (Some(instance_attrs), self.static_vertices.count(), segments)
        };

        let shader = context.shader_key(self.program, uniforms, self.projection_variant);
        let wall_shader = context.shader_key(self.wall_program, wall_uniforms, self.projection_variant);
        let color = Some(ColorMode::alpha_blended());

        let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
        vertex_attrs.set_shared(
            "pos",
            SharedAttribute::new(
                vertices.clone(),
                AttributeDataType::Short2,
                offset_of!(FillExtrusionLayoutVertex, a1) as u32,
                stride,
            ),
        );
        vertex_attrs.set_shared(
            "decimals_ed",
            SharedAttribute::new(
                vertices.clone(),
                AttributeDataType::UShort2,
                offset_of!(FillExtrusionLayoutVertex, a2) as u32,
                stride,
            ),
        );

        let atlases = self.has_pattern.then(|| tile_bucket.atlas_textures()).map(|atlases| {
            AtlasBinding::new(
                Some(atlases.clone()),
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
        let roofs = SharedGeometry {
            vertices: &vertex_attrs,
            instances: None,
            vertex_count,
            mode: DrawMode::Triangles(),
            indexes: &triangles,
            segments: &bucket.triangle_segments,
        };
        if !bucket.triangles.is_empty() && vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(&shader, false, None, false, &roofs, atlases.as_ref()));
            }
            draws.push(extrusion_draw(&shader, false, color, clip, &roofs, atlases.as_ref()));
        }
        // A tile with one vertex has no wall.
        let walls_geometry = SharedGeometry {
            vertices: &wall_attrs,
            instances: wall_instance_attrs.as_ref(),
            vertex_count: wall_vertex_count,
            mode: if self.walls == Walls::Instanced { DrawMode::TriangleStrip() } else { DrawMode::Triangles() },
            indexes: self.static_indices,
            segments: &wall_segments,
        };
        if walls > 0 && wall_vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(&wall_shader, true, None, false, &walls_geometry, atlases.as_ref()));
            }
            draws.push(extrusion_draw(&wall_shader, true, color, clip, &walls_geometry, atlases.as_ref()));
        }
        if self.walls == Walls::Pulled {
            for draw in draws.iter_mut().filter(|draw| draw.walls) {
                draw.objects.read_as_storage(WALL_VERTICES_ID, Some(vertices.clone()));
                draw.objects.read_as_storage(WALL_PAINT_ID, paint.cloned());
            }
        }

        ExtrusionTile {
            draws,
            paint_layout,
            atlases,
            heights: GroundHeights::default(),
            quad_heights: GroundHeights::default(),
        }
    }
}

/// The paint values of the walls' own quads, which the bucket's upload made
/// of its paint values before those moved to the vectors the roofs draw,
/// unless feature state rewrites them.
fn wall_paint(
    context: &mut Context,
    geometry: &BucketGeometry,
    binders: &FillExtrusionBinders,
    walls: usize,
) -> Option<SharedVertices> {
    let buffer = &binders.interleaved_vertex_buffer;
    if binders.depends_on_feature_state() {
        let paint = wall_quad_paint(&buffer.shared_vertex_vector.data(), buffer.stride, walls);
        Some(context.add_vertices(paint))
    } else {
        derived_vertices(geometry, &buffer.shared_vertex_vector)
    }
}

impl TileBuilder<'_> {
    /// Points the draws of `cached`, the tile's, at the paint values of its
    /// bucket's `binders` that feature state rewrote.
    fn refresh_paint(
        &self,
        context: &mut Context,
        cached: &mut ExtrusionTile,
        tile_bucket: LayerBucket<'_, FillExtrusion>,
        binders: &FillExtrusionBinders,
        evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    ) {
        let geometry = tile_bucket.geometry();
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        let walls = tile_bucket.bucket.vertices.len().saturating_sub(1);
        let quad_paint = (self.walls == Walls::Quads).then(|| wall_paint(context, geometry, binders, walls)).flatten();
        for draw in &mut cached.draws {
            let attrs = draw.objects.paint_attributes_mut();
            binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
            if let (Some(paint), Some(quad_paint)) = (&paint, quad_paint.as_ref().filter(|_| draw.walls)) {
                read_quad_paint(attrs, paint, quad_paint);
            }
            if self.walls == Walls::Pulled && draw.walls {
                cached.paint_layout = paint_layout(attrs);
                draw.objects.read_as_storage(WALL_PAINT_ID, paint.clone());
            }
        }
    }
}

impl RenderFillExtrusionLayer {
    /// The entries in the layer's storage buffers of `draw`, a draw of
    /// `tile`, whose bucket's binders are `binders` and whose pulled walls
    /// find their paint values by `paint_layout`, with the layer's values
    /// `layer`.
    fn uniforms(
        layer: &EvaluatedLayer<FillExtrusion>,
        tile_id: OverscaledTileID,
        tile_bucket: LayerBucket<'_, FillExtrusion>,
        binders: &FillExtrusionBinders,
        draw: &ExtrusionDraw,
        paint_layout: u32,
        parameters: &PaintParameters<'_>,
    ) -> ExtrusionUniforms {
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        // The near-clipped projection keeps the depth buffer's precision for
        // the extrusions, but over terrain they test the frame's depth of the
        // ground, which the surface's projection draws, and the frame reports
        // that projection (`frame_projection::compute`).
        let near_clipped = !parameters.terrain;
        let evaluated = &layer.paint;
        let state = parameters.view;
        let zoom = state.zoom() as f32;
        let tile_id = tile_id.to_unwrapped();

        let mut projection = projection_data(
            &tile_id,
            parameters,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            near_clipped,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
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
        if parameters.context.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !state.is_globe() {
            zero_to_one_clip_z(&mut projection.main_matrix);
            zero_to_one_clip_z(&mut projection.fallback_matrix);
        }

        let int_zoom = state.integer_zoom();
        let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
        let zoom_scale = mln_map_types::zoom_scale(f64::from(tile_id.canonical.z));
        let nearest_zoom_scale = mln_map_types::zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z));
        let tile_size_at_nearest_zoom = (TILE_SIZE_D * nearest_zoom_scale).floor();
        let pixel_x = (tile_size_at_nearest_zoom
            * (f64::from(tile_id.canonical.x) + f64::from(tile_id.wrap) * zoom_scale)) as i32;
        let pixel_y = (tile_size_at_nearest_zoom * f64::from(tile_id.canonical.y)) as i32;
        let num_tiles = 2f64.powi(i32::from(tile_id.canonical.z));
        let height_factor = (-num_tiles / TILE_SIZE_D / 8.0) as f32;

        let texture = draw.objects.texture(IMAGE_SLOT).and_then(|texture| parameters.context.texture(texture));
        let texture_size = texture.map(Texture2D::size).unwrap_or_default();
        let pattern = evaluated.fill_extrusion_pattern.faded();
        let tlbr = |id: &str| tile_bucket.pattern(id).map_or([0.0; 4], |position| position.tlbr().map(f32::from));

        let pattern_t = binders.fill_extrusion_pattern.interpolation_factor(zoom).0;
        ExtrusionUniforms {
            draw_ubo: FillExtrusionDrawableUBO {
                matrix: cast_matrix(&projection.fallback_matrix),
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
            },
            tile_props: FillExtrusionTilePropsUBO {
                pattern_from: tlbr(pattern.from.id()),
                pattern_to: tlbr(pattern.to.id()),
                texsize: [texture_size.width as f32, texture_size.height as f32],
                pad1: 0.0,
                pad2: 0.0,
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// Writes the layer's buffers of the frame's draws, which draw at
    /// `opacity`.
    fn write_uniforms(
        layer: &EvaluatedLayer<FillExtrusion>,
        writer: &mut UniformWriter<'_, ExtrusionUniforms>,
        light: &EvaluatedLight,
        state: &View,
        opacity: f32,
    ) {
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        let props = FillExtrusionPropsUBO {
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
        };
        writer.uniform(PROPS_ID, &props);
        writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
        writer.storage(TILE_PROPS_ID, |uniforms| uniforms.tile_props);
        writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;

    use super::*;
    use crate::buckets::fill_extrusion_bucket::wall_quad_vertices;
    use crate::gfx::segment::MAX_SEGMENT_VERTICES;

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
        let corners = RenderStaticData::fill_extrusion_vertices();
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

    // The segments draw every wall, each from the start of the indices, which
    // address no more vertices than 16 bits do.
    #[test]
    fn wall_segments_draw_every_wall() {
        let indices = RenderStaticData::fill_extrusion_wall_indices();
        assert_eq!(indices[..12], [0, 2, 1, 1, 2, 3, 4, 6, 5, 5, 6, 7]);
        let most = indices.len() / 6;
        assert_eq!(usize::from(*indices.iter().max().unwrap()), 4 * most - 1);
        assert!(4 * most <= MAX_SEGMENT_VERTICES);

        assert_eq!(RenderStaticData::fill_extrusion_wall_segments(0).len(), 0);
        let segments = RenderStaticData::fill_extrusion_wall_segments(most + 2);
        let lengths: Vec<_> = segments
            .iter()
            .map(|s| (s.vertex_offset, s.index_offset, s.vertex_length, s.index_length, s.instance_count))
            .collect();
        assert_eq!(lengths, [(0, 0, 4 * most, 6 * most, 1), (4 * most, 0, 8, 12, 1)]);
    }
}
