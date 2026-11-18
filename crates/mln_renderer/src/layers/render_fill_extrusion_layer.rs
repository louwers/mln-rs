// Each tile draws its roofs from the bucket's triangles and its walls as
// instances of one static quad, an instance per bucket vertex. The WebGPU
// and Vulkan walls also read the next vertex as attributes of their own
// (`ID_FILL_EXTRUSION_NEXT_*`), from the same buffer one vertex further on,
// so a tile draws an instance for every vertex but the last, which ends a
// ring and has no wall.
//
// A device that rasterizes on the CPU runs its vertex pipeline once per
// instance, so there each wall is a quad of its own in the vertex buffer:
// every outline vertex is in it four times, once with each corner of the
// quad (`FillExtrusionWallVertex`), and a wall reads the next outline vertex
// from the same buffer four vertices further on. The wall shaders run the
// instanced ones' code with every attribute per vertex: the same vertices in
// the same order, drawn without instances.

use std::cell::RefCell;
use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::fill_extrusion_layer_ubo::{
    FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_DRAWABLE_UBO,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PROPS_UBO, ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_program::ShaderProgramPtr;
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::FillExtrusion;
use mln_style::layers::fill_extrusion_layer_properties::FillExtrusionPaintProperties;
use mln_style_types::AlignmentType;
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::ClipDepthRange;
use mln_util::size::Size;

use super::{cast_matrix, shared_vertices};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, derived_vertices, paint_vertices};
use crate::buckets::fill_extrusion_bucket::{
    FillExtrusionBinders, FillExtrusionBucket, FillExtrusionLayoutSpec, FillExtrusionLayoutVertex,
    FillExtrusionStaticVertex, FillExtrusionWallVertex, wall_quad_paint, wall_quad_vertices,
};
use crate::draw::{DrawGeometry, DrawObjects, copy_shared_attributes};
use crate::draw_cache::{
    DrawDepth, DrawList, DrawPriority, DrawSlots, FrameDraw, FrameDraws, LayerDrawing, UniformWriter,
};
use crate::frame_projection::zero_to_one_clip_z;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{PatternLayout, uses_pattern};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    LayerParts, LayerSource, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// A draw the layer keeps of a tile: of its roofs or its walls, into depth
/// only or in color.
struct ExtrusionDraw {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    /// 0 for a draw into depth only, which draws before those in color.
    priority: DrawPriority,
    /// Whether it tests the stencil value the layer's 3D draws share, which
    /// keeps the draws in color from blending a pixel twice.
    clip: bool,
}

/// What the layer keeps of a tile: the draws of its roofs and walls, as they
/// were built for the tile's bucket.
pub(crate) struct ExtrusionTile {
    draws: Vec<ExtrusionDraw>,
    /// Gives the pattern draws the texture of the tile's icon atlas.
    atlases: Option<AtlasBinding>,
}

/// Each draw in the slot of its index.
impl DrawSlots for ExtrusionTile {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.draws.get_mut(slot as usize).map(|draw| &mut draw.objects)
    }

    fn slot_with_atlases(&mut self, slot: u32) -> Option<(&mut DrawObjects, Option<&AtlasBinding>)> {
        Some((&mut self.draws.get_mut(slot as usize)?.objects, self.atlases.as_ref()))
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
    indexes: &'a IndexVectorBasePtr,
    segments: &'a [SegmentBase],
}

fn read_paint_properties(
    attrs: &mut VertexAttributeArray,
    binders: &FillExtrusionBinders,
    evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    shared: &VertexVectorBasePtr,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    properties_as_uniforms.0.clear();
    properties_as_uniforms.1.clear();
    let buffer = &binders.interleaved_vertex_buffer;
    let mut id = ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_base,
        buffer,
        shared,
        &["a_base"],
        evaluated.fill_extrusion_base.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.fill_extrusion_color.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_height,
        buffer,
        shared,
        &["a_height"],
        evaluated.fill_extrusion_height.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_extrusion_pattern,
        buffer,
        shared,
        &["a_pattern_to", "a_pattern_from"],
        evaluated.fill_extrusion_pattern.is_constant(),
        properties_as_uniforms,
        &mut id,
    );
}

/// Whether the walls draw as instances of the static quad, rather than as
/// quads of their own, which a device that rasterizes on the CPU draws.
pub(crate) fn instanced_walls(context: &crate::gfx::context::Context) -> bool {
    !context.caps().cpu_rasterizer
}

/// Points each attribute of `attrs` that reads `paint` at `quad_paint`
/// (`wall_quad_paint`), which has the same layout. The binders apply their
/// attributes only to a vector of the bucket's vertices.
fn read_quad_paint(attrs: &mut VertexAttributeArray, paint: &VertexVectorBasePtr, quad_paint: &VertexVectorBasePtr) {
    for id in 0..attrs.allocated_size() {
        let Some(attr) =
            attrs.get(id).filter(|attr| attr.get_shared_raw_data().is_some_and(|data| Rc::ptr_eq(data, paint)))
        else {
            continue;
        };
        let (offset, vertex_offset) = (attr.get_shared_offset(), attr.get_shared_vertex_offset());
        let (stride, type_) = (attr.get_shared_stride(), attr.get_shared_type());
        if let Some(attr) = attrs.set(id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(quad_paint)), offset, vertex_offset, stride, type_);
        }
    }
}

/// Reads each wall vertex's corner and its wall's outline vertex from
/// `quads` (`wall_quad_vertices`), and the next outline vertex from the same
/// buffer four vertices further on.
fn read_wall_quads(attrs: &mut VertexAttributeArray, quads: &VertexVectorBasePtr) {
    let stride = size_of::<FillExtrusionWallVertex>() as u32;
    let corner = offset_of!(FillExtrusionWallVertex, corner) as u32;
    let pos = offset_of!(FillExtrusionWallVertex, vertex) as u32;
    let decimals_ed = pos + offset_of!(FillExtrusionLayoutVertex, a2) as u32;
    for (id, offset, vertex_offset, type_) in [
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, corner, 0, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, pos, 0, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, decimals_ed, 0, AttributeDataType::UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, pos, 4, AttributeDataType::Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, decimals_ed, 4, AttributeDataType::UShort2),
    ] {
        if let Some(attr) = attrs.set(id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(quads)), offset, vertex_offset, stride, type_);
        }
    }
}

/// Reads each vertex's corner of the static quad from `corners`, and each
/// instance's vertex and the next one from `vertices`, the bucket's.
fn read_wall_instances(
    attrs: &mut VertexAttributeArray,
    instance_attrs: &mut VertexAttributeArray,
    corners: &VertexVectorBasePtr,
    vertices: &VertexVectorBasePtr,
) {
    let static_stride = size_of::<FillExtrusionStaticVertex>() as u32;
    if let Some(attr) = attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(corners)), 0, 0, static_stride, AttributeDataType::Short2);
    }
    let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
    // An instance's outline vertex, then the next one.
    for (vertex_offset, pos_id, decimals_ed_id) in [
        (0, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        (1, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
    ] {
        if let Some(attr) = instance_attrs.set(pos_id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(vertices)), 0, vertex_offset, stride, AttributeDataType::Short2);
        }
        if let Some(attr) = instance_attrs.set(decimals_ed_id, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(Some(Rc::clone(vertices)), 4, vertex_offset, stride, AttributeDataType::UShort2);
        }
    }
}

/// A draw of `geometry` with `shader`, into depth only or, `in_color`, in
/// color, which binds the textures of `atlases`.
fn extrusion_draw(
    shader: &ShaderProgramPtr,
    in_color: bool,
    clip: bool,
    geometry: &SharedGeometry<'_>,
    atlases: Option<&AtlasBinding>,
) -> ExtrusionDraw {
    let draw_geometry = DrawGeometry {
        vertices: copy_shared_attributes(geometry.vertices),
        vertex_count: if geometry.segments.is_empty() { 0 } else { geometry.vertex_count },
        mode: DrawMode::Triangles(),
        indexes: Rc::clone(geometry.indexes),
        segments: geometry.segments.iter().map(SegmentBase::copy).collect(),
        instances: geometry.instances.map(copy_shared_attributes),
    };
    let color_mode = if in_color { ColorMode::alpha_blended() } else { ColorMode::disabled() };
    let textures = atlases.map(|atlases| atlases.textures(shader)).unwrap_or_default();
    let objects = DrawObjects::new(Rc::clone(shader), color_mode, CullFaceMode::back_ccw(), draw_geometry, textures);
    ExtrusionDraw { id: SimpleIdentity::new(), objects, priority: DrawPriority::from(in_color), clip }
}

impl LayerType for FillExtrusion {
    type Layout = PatternLayout<FillExtrusionLayoutSpec>;
    type Bucket = FillExtrusionBucket;
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
    drawing: LayerDrawing<ExtrusionTile>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<ExtrusionUniforms>,

    fill_extrusion_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_group: Option<ShaderGroupPtr>,
    /// The walls' shaders: the instanced ones, or the wall shaders where the
    /// walls are quads of their own.
    fill_extrusion_wall_group: Option<ShaderGroupPtr>,
    fill_extrusion_pattern_wall_group: Option<ShaderGroupPtr>,

    /// The wall quad (`RenderStaticData::fill_extrusion_vertices`) of the
    /// instanced walls.
    static_data_vertices: Option<VertexVectorBasePtr>,
    /// The triangles of the wall quad, or of the walls' own quads
    /// (`RenderStaticData::fill_extrusion_wall_indices`).
    static_data_indices: Option<IndexVectorBasePtr>,
}

impl RenderFillExtrusionLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<FillExtrusion>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderFillExtrusionLayer {
            base,
            values,
            drawing: LayerDrawing::default(),
            draw_list: DrawList::default(),
            fill_extrusion_group: None,
            fill_extrusion_pattern_group: None,
            fill_extrusion_wall_group: None,
            fill_extrusion_pattern_wall_group: None,
            static_data_vertices: None,
            static_data_indices: None,
        }
    }
}

impl TypedLayer for RenderFillExtrusionLayer {
    type Type = FillExtrusion;
    type Tile = ExtrusionTile;
    type Shared = ();

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }
}

impl RenderLayer for RenderFillExtrusionLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let properties = self.values.evaluate(parameters);

        self.base.passes = if *properties.paint.fill_extrusion_opacity.uniform() > 0.0 {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn is_3d(&self) -> bool {
        true
    }

    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        _zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        let evaluated = &self.values.evaluated.paint;
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(query_geometry),
            geometry,
        )
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
        self.update_projection_variant(state);

        let render_tiles = match tiles {
            Some(tiles) if !tiles.is_empty() && self.base.passes != RenderPass::None => tiles,
            _ => {
                self.remove_all_draws();
                return;
            }
        };
        let frame = self.drawing.frame.get_or_insert_with(FrameDraws::new);

        if self.fill_extrusion_group.is_none() {
            self.fill_extrusion_group = shaders.shader_group("FillExtrusionShader");
        }
        if self.fill_extrusion_pattern_group.is_none() {
            self.fill_extrusion_pattern_group = shaders.shader_group("FillExtrusionPatternShader");
        }
        // A device that rasterizes on the CPU draws each wall as a quad of
        // its own.
        let instanced_walls = instanced_walls(context);
        if self.fill_extrusion_wall_group.is_none() {
            let name = if instanced_walls { "FillExtrusionInstancedShader" } else { "FillExtrusionWallShader" };
            self.fill_extrusion_wall_group = shaders.shader_group(name);
        }
        if self.fill_extrusion_pattern_wall_group.is_none() {
            let name =
                if instanced_walls { "FillExtrusionPatternInstancedShader" } else { "FillExtrusionPatternWallShader" };
            self.fill_extrusion_pattern_wall_group = shaders.shader_group(name);
        }

        let has_pattern = !self.values.unevaluated.fill_extrusion_pattern.is_undefined();
        let (shader_group, wall_shader_group) = if has_pattern {
            (&self.fill_extrusion_pattern_group, &self.fill_extrusion_pattern_wall_group)
        } else {
            (&self.fill_extrusion_group, &self.fill_extrusion_wall_group)
        };
        let (Some(shaders), Some(wall_shaders)) = (shader_group.clone(), wall_shader_group.clone()) else {
            self.remove_all_draws();
            return;
        };

        let static_vertices = Rc::clone(
            self.static_data_vertices
                .get_or_insert_with(|| shared_vertices(&RenderStaticData::fill_extrusion_vertices())),
        );
        let static_indices = Rc::clone(self.static_data_indices.get_or_insert_with(|| {
            Rc::new(RefCell::new(if instanced_walls {
                IndexVectorBase::new(RenderStaticData::fill_extrusion_triangle_indices())
            } else {
                IndexVectorBase::new_released(RenderStaticData::fill_extrusion_wall_indices())
            }))
        }));
        let mut builder = TileBuilder {
            shaders,
            wall_shaders,
            projection_variant: self.base.projection_variant,
            instanced_walls,
            static_vertices,
            static_indices,
            has_pattern,
            // The layer draws into depth before it draws in color where it
            // is translucent or has a pattern.
            depth_pass: *self.values.evaluated.paint.fill_extrusion_opacity.uniform() < 1.0 || has_pattern,
            first_shaders: None,
        };

        let layer = &*self.values.evaluated;
        for tile in render_tiles.iter() {
            let tile_id = tile.id();

            let render_data = super::layer_bucket_for_pass(tile, &*self.base.style, RENDER_PASS);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.drawing.caches.drop_cache(&tile_id);
                continue;
            };
            let Some(bucket) = render_data.bucket::<FillExtrusion>() else {
                continue;
            };
            if self.drawing.caches.bucket(&tile_id).is_some_and(|prev| prev != bucket.base.bucket_id) {
                // The tile's draws were built from another bucket.
                self.drawing.caches.drop_cache(&tile_id);
            }
            self.drawing.caches.set_bucket(&tile_id, bucket.base.bucket_id);

            let Some(binders) = bucket.paint_property_binders.get(self.base.style.base().id.as_str()) else {
                continue;
            };

            // The draws keep the paint values they were built with, so
            // feature state doesn't reach an extrusion: the tile's changes go
            // unread.
            if self.drawing.caches.cache_mut(&tile_id).is_none() {
                let built = builder.build(tile, bucket, binders, &layer.paint);
                self.drawing.caches.set_cache(&tile_id, built);
            }
            let Some(cached) = self.drawing.caches.cache(&tile_id) else {
                continue;
            };
            for (slot, draw) in cached.draws.iter().enumerate() {
                let frame_draw = FrameDraw {
                    priority: draw.priority,
                    pass: RENDER_PASS,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    clip: draw.clip,
                    ..FrameDraw::new(Some(tile_id), slot as u32, draw.id)
                };
                self.draw_list.push(frame_draw, Self::uniforms(layer, tile, binders, &draw.objects, paint_parameters));
            }
        }

        frame.set_stencil_tiles(render_tiles.render_tiles().map(|tile| tile.id));
        self.draw_list.finish(frame, context, |writer| Self::write_uniforms(layer, writer, paint_parameters));
    }
}

/// What building the draws of an update's tiles reads of the layer.
struct TileBuilder {
    /// The shaders of the roofs and of the walls.
    shaders: ShaderGroupPtr,
    wall_shaders: ShaderGroupPtr,
    projection_variant: mln_gfx::projection_variant::ProjectionVariant,
    /// Whether the walls are instances of the static quad, rather than quads
    /// of their own.
    instanced_walls: bool,
    /// The static quad, and the indexes the walls draw.
    static_vertices: VertexVectorBasePtr,
    static_indices: IndexVectorBasePtr,
    has_pattern: bool,
    /// Whether the layer draws into depth before it draws in color.
    depth_pass: bool,
    /// The shaders of the roofs and walls of the first tile built, which the
    /// others take.
    first_shaders: Option<(ShaderProgramPtr, ShaderProgramPtr)>,
}

impl TileBuilder {
    /// The draws of `tile`'s `bucket`, laid out with the layer's `evaluated`
    /// values: of its roofs, then of its walls, each into depth, where the
    /// layer draws it, then in color.
    fn build(
        &mut self,
        tile: FrameTile<'_>,
        bucket: &FillExtrusionBucket,
        binders: &FillExtrusionBinders,
        evaluated: &FillExtrusionPaintProperties::PossiblyEvaluated,
    ) -> ExtrusionTile {
        let geometry = tile.bucket_geometry();
        let vertex_count = bucket.vertices.len();
        // A wall from each vertex to the next, but from the last, which
        // ends a ring.
        let walls = vertex_count.saturating_sub(1);
        let buffer = &binders.interleaved_vertex_buffer;
        let depends_on_feature_state = binders.depends_on_feature_state();
        // The walls' own quads, which the bucket's upload made of its
        // vertices and paint values before those moved to the vectors the
        // roofs draw.
        let quads = (!self.instanced_walls).then(|| {
            let vertices = derived_vertices(geometry, &bucket.vertices, wall_quad_vertices);
            let paint_of = |paint: &[u8]| wall_quad_paint(paint, buffer.stride, walls);
            let paint = if depends_on_feature_state {
                paint_of(&buffer.shared_vertex_vector.data())
            } else {
                derived_vertices(geometry, &buffer.shared_vertex_vector, paint_of)
            };
            (vertices, paint)
        });

        // The roofs read the paint values per vertex, the instanced walls per
        // instance, from the same buffer.
        let paint = paint_vertices(geometry, buffer, depends_on_feature_state);
        let vertices = bucket_vertices(geometry, &bucket.vertices);
        let mut properties_as_uniforms = StringIDSetsPair::default();
        let mut wall_properties_as_uniforms = StringIDSetsPair::default();
        let mut vertex_attrs = VertexAttributeArray::new();
        read_paint_properties(&mut vertex_attrs, binders, evaluated, &paint, &mut properties_as_uniforms);
        let mut wall_attrs = VertexAttributeArray::new();
        let (wall_instance_attrs, wall_vertex_count, wall_segments) = if let Some((quad_vertices, quad_paint)) = &quads
        {
            read_paint_properties(&mut wall_attrs, binders, evaluated, &paint, &mut wall_properties_as_uniforms);
            read_quad_paint(&mut wall_attrs, &paint, quad_paint);
            read_wall_quads(&mut wall_attrs, quad_vertices);
            (None, 4 * walls, RenderStaticData::fill_extrusion_wall_segments(walls))
        } else {
            let mut instance_attrs = VertexAttributeArray::new();
            read_paint_properties(&mut instance_attrs, binders, evaluated, &paint, &mut wall_properties_as_uniforms);
            read_wall_instances(&mut wall_attrs, &mut instance_attrs, &self.static_vertices, &vertices);
            let segments = vec![RenderStaticData::fill_extrusion_segment(walls)];
            (Some(instance_attrs), self.static_vertices.get_raw_count(), segments)
        };

        let (shader, wall_shader) = self.first_shaders.get_or_insert_with(|| {
            (
                self.shaders.shader(&properties_as_uniforms, self.projection_variant),
                self.wall_shaders.shader(&wall_properties_as_uniforms, self.projection_variant),
            )
        });

        let stride = size_of::<FillExtrusionLayoutVertex>() as u32;
        if let Some(attr) = vertex_attrs.set(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
        {
            // offsetof(FillExtrusionLayoutVertex, a1)
            attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 0, 0, stride, AttributeDataType::Short2);
        }
        if let Some(attr) = vertex_attrs.set(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
        {
            // offsetof(FillExtrusionLayoutVertex, a2)
            attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 4, 0, stride, AttributeDataType::UShort2);
        }

        let atlases = tile.atlas_textures().filter(|_| self.has_pattern).map(|atlases| {
            AtlasBinding::new(
                Some(Rc::clone(atlases)),
                None,
                Some(ID_FILL_EXTRUSION_IMAGE_TEXTURE),
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
            indexes: &bucket_indexes(geometry, &bucket.triangles),
            segments: &bucket.triangle_segments,
        };
        if !bucket.triangles.is_empty() && vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(shader, false, false, &roofs, atlases.as_ref()));
            }
            draws.push(extrusion_draw(shader, true, clip, &roofs, atlases.as_ref()));
        }
        // A tile with one vertex has no wall.
        let walls_geometry = SharedGeometry {
            vertices: &wall_attrs,
            instances: wall_instance_attrs.as_ref(),
            vertex_count: wall_vertex_count,
            indexes: &self.static_indices,
            segments: &wall_segments,
        };
        if walls > 0 && wall_vertex_count > 0 {
            if self.depth_pass {
                draws.push(extrusion_draw(wall_shader, false, false, &walls_geometry, atlases.as_ref()));
            }
            draws.push(extrusion_draw(wall_shader, true, clip, &walls_geometry, atlases.as_ref()));
        }

        ExtrusionTile { draws, atlases }
    }
}

impl RenderFillExtrusionLayer {
    /// The entries in the layer's storage buffers of a draw of `objects`, a
    /// draw of `tile`, whose bucket's binders are `binders`, with the layer's
    /// values `layer`.
    fn uniforms(
        layer: &EvaluatedLayer<FillExtrusion>,
        tile: FrameTile<'_>,
        binders: &FillExtrusionBinders,
        objects: &DrawObjects,
        parameters: &PaintParameters<'_>,
    ) -> ExtrusionUniforms {
        const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
        const NEAR_CLIPPED: bool = true;
        let evaluated = &layer.paint;
        let state = parameters.state;
        let zoom = state.get_zoom() as f32;
        let tile_id = tile.id().to_unwrapped();

        let mut projection = get_projection_data(
            &tile_id,
            parameters,
            *evaluated.fill_extrusion_translate.uniform(),
            *evaluated.fill_extrusion_translate_anchor.uniform(),
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0 (`frame_projection`). The globe projects in its
        // shaders.
        if parameters.context.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !state.is_globe_rendering() {
            zero_to_one_clip_z(&mut projection.main_matrix);
            zero_to_one_clip_z(&mut projection.fallback_matrix);
        }

        let int_zoom = state.get_integer_zoom();
        let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
        let zoom_scale = state.zoom_scale(f64::from(tile_id.canonical.z));
        let nearest_zoom_scale = state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z));
        let tile_size_at_nearest_zoom = (TILE_SIZE_D * nearest_zoom_scale).floor();
        let pixel_x = (tile_size_at_nearest_zoom
            * (f64::from(tile_id.canonical.x) + f64::from(tile_id.wrap) * zoom_scale)) as i32;
        let pixel_y = (tile_size_at_nearest_zoom * f64::from(tile_id.canonical.y)) as i32;
        let num_tiles = 2f64.powi(i32::from(tile_id.canonical.z));
        let height_factor = (-num_tiles / TILE_SIZE_D / 8.0) as f32;

        let texture = objects.texture(ID_FILL_EXTRUSION_IMAGE_TEXTURE);
        let texture_size = texture.map_or(Size::new(0, 0), |texture| texture.borrow().size());
        let pattern = evaluated.fill_extrusion_pattern.faded();
        let tlbr = |id: &str| tile.pattern(id).map_or([0.0; 4], |position| position.tlbr().map(f32::from));

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
                pad1: 0.0,
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

    /// Writes the layer's buffers of the frame's draws.
    fn write_uniforms(
        layer: &EvaluatedLayer<FillExtrusion>,
        writer: &mut UniformWriter<'_, ExtrusionUniforms>,
        parameters: &PaintParameters<'_>,
    ) {
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;
        let light = &parameters.evaluated_light;
        let props = FillExtrusionPropsUBO {
            color: evaluated
                .fill_extrusion_color
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_color())
                .to_float_array(),
            light_color: FillExtrusionBucket::light_color(light),
            pad1: 0.0,
            light_position: FillExtrusionBucket::light_position(light, parameters.state),
            base: evaluated
                .fill_extrusion_base
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_base()),
            height: evaluated
                .fill_extrusion_height
                .constant_or(&FillExtrusionPaintProperties::Defaults::fill_extrusion_height()),
            light_intensity: FillExtrusionBucket::light_intensity(light),
            vertical_gradient: if *evaluated.fill_extrusion_vertical_gradient.uniform() { 1.0 } else { 0.0 },
            opacity: *evaluated.fill_extrusion_opacity.uniform(),
            fade: crossfade.t,
            from_scale: crossfade.from_scale,
            to_scale: crossfade.to_scale,
            pad2: 0.0,
        };
        writer.uniform(ID_FILL_EXTRUSION_PROPS_UBO, &props);
        writer.storage(ID_FILL_EXTRUSION_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
        writer.storage(ID_FILL_EXTRUSION_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
        writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
    }
}

#[cfg(test)]
mod tests {
    use geometry::Point;
    use mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES;

    use super::*;

    // A wall's quad is the static quad's corners, each with the wall's
    // vertex, and the next four are the next vertex's. Its paint values are
    // its instance's, in each corner.
    #[test]
    fn wall_quads_are_the_instances() {
        let vertices: Vec<FillExtrusionLayoutVertex> = (0..4)
            .map(|i| FillExtrusionBucket::layout_vertex(Point::new(f64::from(i) + 0.5, 1.0), i as u16, i == 3))
            .collect();
        let quads = wall_quad_vertices(&vertices);
        let quads = quads.get_raw_data();
        let quads: &[FillExtrusionWallVertex] = bytemuck::cast_slice(&quads);
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
        assert_eq!(*quad_paint.get_raw_data(), *expected);
        assert_eq!(wall_quad_paint(&[], 0, 3).get_raw_count(), 0);
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
