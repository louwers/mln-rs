// A tile's draws read the bucket's layout vertices and copies of what
// placement lays out. When placement, sorting or feature state change the
// bucket (the changes the tile's entry in the layer's `TileMap` collects),
// the draw update replaces just those attributes (opacity, dynamic
// positions, binders) and the triangle indices of the tile's draws.

use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::{Projection, ProjectionData};
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::error::Error;
use mln_style::hash::HashMap;
use mln_style::layer::{ErasedLayerStyle as _, LayerStyle};
use mln_style::layers::Symbol;
use mln_style::layers::symbol_layer::SymbolLayerPaintPropertyOverrides;
use mln_style::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties};
use mln_style::types::{AlignmentType, IconTextFitType, SymbolPlacementType, TextRotationAlignmentType};
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use super::cast_matrix;
use crate::bucket::Bucket;
use crate::bucket_geometry::{BucketGeometry, bucket_vertices, copied_indexes, paint_vertices};
use crate::buckets::symbol_bucket::{
    Buffer, PaintProperties, SymbolBucket, SymbolDynamicLayoutVertex, SymbolOpacityVertex,
};
use crate::draw::{DrawGeometry, DrawObjects, copy_shared_attributes};
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::segment::SegmentBase;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::{SharedVertices, TypedVertices};
use crate::layer_drawing::{
    BucketChanges, DrawGroup, DrawList, DrawObjectsMut, DrawOrder, FrameDraw, LayerDrawing, LayerDraws, TileMap,
    UniformWriter,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layers::symbol::programs;
use crate::layers::symbol::ubo::{SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::symbol_layout::SymbolLayout;
use crate::layout::symbol_projection::{gl_coord_matrix, label_plane_matrix};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, BucketKey, BucketPlacementData, LayerBucket, LayerUpdateParameters, LayerValues, Parts, PartsMut,
    PlacedTile, RenderLayer, RenderLayerBase, SymbolTranslate, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, TileOrder};
use crate::render_tile::{FrameTile, RenderTile};
use crate::terrain::FrameGround;
use crate::tile_matrix::{projection_data, to_projection_ubo};

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::SYMBOL_SDF.buffer_id("drawableVector");
const TILE_PROPS_ID: usize = programs::SYMBOL_SDF.buffer_id("tilePropsVector");
const PROPS_ID: usize = programs::SYMBOL_SDF.buffer_id("props");

/// The slots of the programs' textures that the draws bind.
const GROUND_DEPTH_SLOT: usize = programs::SYMBOL_SDF.texture_slot("ground_depth");
const TEXTURE_SLOT: usize = programs::SYMBOL_SDF.texture_slot("texture");
const TEXTURE_ICON_SLOT: usize = programs::SYMBOL_TEXT_AND_ICON.texture_slot("texture_icon");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolType {
    Text,
    IconRGBA,
    IconSDF,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolPropertyValues {
    pub(crate) pitch_alignment: AlignmentType,
    /// `Map` when the symbols rotate with the map, otherwise `Viewport`.
    pub(crate) rotation_alignment: AlignmentType,
    /// Whether the symbols' glyphs or icons are placed along their lines.
    pub(crate) along_line: bool,
}

/// What a symbol draw draws, which its uniforms read.
#[derive(Clone, Copy, Debug)]
struct SymbolDrawData {
    is_halo: bool,
    bucket_variable_placement: bool,
    symbol_type: SymbolType,
    pitch_alignment: AlignmentType,
    rotation_alignment: AlignmentType,
    along_line: bool,
    text_fit: IconTextFitType,
    is_offset: bool,
    /// The bucket segment the draw draws, for a layer that sorts its
    /// features by key, or all of them.
    segment: Option<usize>,
}

#[must_use]
pub(crate) fn icon_property_values(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> SymbolPropertyValues {
    let rotation_alignment = *layout.icon_rotation_alignment.uniform();
    SymbolPropertyValues {
        pitch_alignment: *layout.icon_pitch_alignment.uniform(),
        rotation_alignment,
        along_line: rotation_alignment == AlignmentType::Map
            && *layout.symbol_placement.uniform() != SymbolPlacementType::Point,
    }
}

#[must_use]
pub(crate) fn text_property_values(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> SymbolPropertyValues {
    let rotation_alignment = *layout.text_rotation_alignment.uniform();
    SymbolPropertyValues {
        pitch_alignment: *layout.text_pitch_alignment.uniform(),
        rotation_alignment: if rotation_alignment == TextRotationAlignmentType::Map {
            AlignmentType::Map
        } else {
            AlignmentType::Viewport
        },
        along_line: rotation_alignment != TextRotationAlignmentType::Viewport
            && *layout.symbol_placement.uniform() != SymbolPlacementType::Point,
    }
}

/// The tile matrix of screen-space symbols.
pub(crate) fn screen_matrix(translation: [f32; 2]) -> DMat4 {
    let extent = f64::from(EXTENT);
    orthographic(0.0, extent, -extent, 0.0, 0.0, 1.0)
        * DMat4::from_translation(DVec3::new(f64::from(translation[0]), f64::from(translation[1]) - extent, 0.0))
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bucket's buffers and the layer's paint properties, and the attributes and uniforms they fill"
)]
fn update_tile_attributes(
    context: &mut Context,
    geometry: &BucketGeometry,
    buffer: &Buffer,
    layer_id: &str,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    uniforms: &mut Uniforms,
) -> PlacedVertices {
    set_binder_attributes(context, geometry, is_text, paint_props, evaluated, attribs, uniforms);
    let mut placed = PlacedVertices::default();
    set_layout_attributes(geometry, buffer, attribs);
    set_projected_pos_attribute(context, buffer, layer_id, attribs, &mut placed.projected);
    set_fade_opacity_attribute(context, buffer, attribs, &mut placed.opacity);
    set_elevation_attribute(context, geometry, buffer, attribs, &mut placed.elevation);
    placed
}

/// The attributes of the data-driven paint properties.
fn set_binder_attributes(
    context: &mut Context,
    geometry: &BucketGeometry,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    uniforms: &mut Uniforms,
) {
    if is_text {
        let binders = &paint_props.text_binders;
        let buffer = &binders.interleaved_vertex_buffer;
        let shared = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        binders.bind(evaluated, shared.as_ref(), attribs, uniforms);
    } else {
        let binders = &paint_props.icon_binders;
        let buffer = &binders.interleaved_vertex_buffer;
        let shared = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        binders.bind(evaluated, shared.as_ref(), attribs, uniforms);
    }
}

/// The attributes of the layout vertices.
fn set_layout_attributes(geometry: &BucketGeometry, buffer: &Buffer, attribs: &mut VertexAttributeArray) {
    let layout_size = size_of::<crate::buckets::symbol_bucket::SymbolLayoutVertex>() as u32;
    let Some(layout_data) = bucket_vertices(geometry, &buffer.attribute_data) else {
        return;
    };
    attribs
        .set_shared("pos_offset", SharedAttribute::new(layout_data.clone(), AttributeDataType::Short4, 0, layout_size));
    attribs.set_shared("data", SharedAttribute::new(layout_data.clone(), AttributeDataType::UShort4, 8, layout_size));
    attribs.set_shared(
        "pixeloffset",
        SharedAttribute::new(layout_data.clone(), AttributeDataType::Short4, 16, layout_size),
    );
    attribs.set_shared(
        "tex_origin",
        SharedAttribute::new(layout_data.clone(), AttributeDataType::UShort2, 24, layout_size),
    );
    set_height_offset_attribute(layout_data, attribs);
}

/// Sets the symbols' heights to the layout vertices' `symbol-height-offset`s,
/// of `layout_data`.
fn set_height_offset_attribute(layout_data: SharedVertices, attribs: &mut VertexAttributeArray) {
    let layout_size = size_of::<crate::buckets::symbol_bucket::SymbolLayoutVertex>() as u32;
    attribs.set_shared("height_offset", SharedAttribute::new(layout_data, AttributeDataType::Float, 28, layout_size));
}

/// Sets the symbols' heights to the heights placement gave them over the
/// ground ([`Buffer::elevation_attribute_data`]), into `placed`, or, where
/// it gave none, to their `symbol-height-offset`s.
fn set_elevation_attribute(
    context: &mut Context,
    geometry: &BucketGeometry,
    buffer: &Buffer,
    attribs: &mut VertexAttributeArray,
    placed: &mut Option<TypedVertices<f32>>,
) {
    let elevations = &buffer.elevation_attribute_data;
    if !elevations.is_empty() {
        set_placed_attribute(context, attribs, "height_offset", elevations, AttributeDataType::Float, placed);
    } else if placed.take().is_some()
        && let Some(layout_data) = bucket_vertices(geometry, &buffer.attribute_data)
    {
        set_height_offset_attribute(layout_data, attribs);
    }
}

/// Sets the dynamic layout vertices, which placement updates: the layer's
/// own when it has them.
fn set_projected_pos_attribute(
    context: &mut Context,
    buffer: &Buffer,
    layer_id: &str,
    attribs: &mut VertexAttributeArray,
    placed: &mut Option<TypedVertices<SymbolDynamicLayoutVertex>>,
) {
    let vertices = buffer.dynamic_attribute_data_for(layer_id);
    set_placed_attribute(context, attribs, "projected_pos", vertices, AttributeDataType::Float4, placed);
}

/// Sets the opacity vertices, which placement updates.
fn set_fade_opacity_attribute(
    context: &mut Context,
    buffer: &Buffer,
    attribs: &mut VertexAttributeArray,
    placed: &mut Option<TypedVertices<SymbolOpacityVertex>>,
) {
    let vertices = &buffer.opacity_attribute_data;
    set_placed_attribute(context, attribs, "fade_opacity", vertices, AttributeDataType::Float, placed);
}

/// Sets attribute `name` of `attribs` to read a copy of `vertices`, which
/// placement updates, as `type_`: into `placed`, the vertices the attribute
/// reads, where they are as many, so that the upload writes them over their
/// buffer, or else into vertices of their own, which `placed` then holds.
fn set_placed_attribute<V: bytemuck::Pod>(
    context: &mut Context,
    attribs: &mut VertexAttributeArray,
    name: &'static str,
    vertices: &[V],
    type_: AttributeDataType,
    placed: &mut Option<TypedVertices<V>>,
) {
    if placed.as_ref().is_some_and(|typed| context.update_vertices(typed, vertices)) {
        return;
    }
    let typed = context.add_dynamic_vertices(vertices);
    attribs.set_shared(name, SharedAttribute::new(typed.shared().clone(), type_, 0, size_of::<V>() as u32));
    *placed = Some(typed);
}

/// The vertices that placement lays out, which the draws of a tile that read
/// the same attributes share, of their positions along lines or at variable
/// anchors, of their opacities and of their heights.
#[derive(Clone, Default)]
struct PlacedVertices {
    projected: Option<TypedVertices<SymbolDynamicLayoutVertex>>,
    opacity: Option<TypedVertices<SymbolOpacityVertex>>,
    /// The heights over the ground, where placement gives them.
    elevation: Option<TypedVertices<f32>>,
}

/// A draw the layer keeps of a tile: the halo or the fill of the text or the
/// icons of one of the bucket's buffers, or of one of its segments.
struct SymbolDraw {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    /// The vertices placement lays out, which the draw's attributes read.
    placed: PlacedVertices,
    data: SymbolDrawData,
    /// Whether placement shows any of the draw's symbols. A draw that shows
    /// none doesn't draw, and doesn't copy the bucket's attributes again
    /// until it shows some.
    visible: bool,
    /// Gives the draw the textures of its tile's atlases.
    atlases: AtlasBinding,
}

/// What tells apart the draws of a tile that read the same attributes: a
/// halo and a fill of one symbol type, and of one segment or all of them.
type AttributesKey = (SymbolType, Option<usize>);

impl SymbolDraw {
    fn attributes_key(&self) -> AttributesKey {
        (self.data.symbol_type, self.data.segment)
    }
}

/// What the layer keeps of a tile: its draws, as they were built for the
/// tile's bucket.
#[derive(Default)]
pub(crate) struct SymbolTile {
    draws: Vec<SymbolDraw>,
}

/// The layer's drawing: what it keeps of each tile, and its draws of the
/// frame, each of the draw at its index among its tile's.
#[derive(Default)]
pub(crate) struct SymbolDrawing {
    tiles: TileMap<SymbolTile>,
    frame: Option<LayerDraws<usize>>,
}

impl SymbolDrawing {
    /// The GPU objects of `draw`, the draw at `index` of its tile, with what
    /// gives them the textures of the tile's atlases.
    fn objects<'a>(tiles: &'a mut TileMap<SymbolTile>, draw: &FrameDraw, index: usize) -> Option<DrawObjectsMut<'a>> {
        let draw = tiles.get_mut(&draw.tile?)?.draws.get_mut(index)?;
        Some(DrawObjectsMut { objects: &mut draw.objects, atlases: Some(&draw.atlases) })
    }

    /// Draws the pass's draws of the layer's draw group, if it has it.
    fn render_frame(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(&mut self.tiles, Self::objects, gpu, parameters);
            order.step(parameters);
        }
    }
}

impl LayerDrawing for SymbolDrawing {
    type Tile = SymbolTile;

    fn tiles(&mut self) -> Option<&mut TileMap<SymbolTile>> {
        Some(&mut self.tiles)
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(frame.frame()));
        }
    }

    fn clear(&mut self) {
        self.tiles.clear();
        self.clear_frame();
    }

    fn begin_frame(&mut self) {
        self.frame.get_or_insert_default().clear();
    }

    fn clear_frame(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Some(frame) = &self.frame {
            frame.upload(gpu, &mut self.tiles, Self::objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        self.render_frame(gpu, parameters, order);
    }
}

/// A draw's entries in the layer's storage buffers.
struct SymbolUniforms {
    draw_ubo: SymbolDrawableUBO,
    tile_props: SymbolTilePropsUBO,
    projection: ProjectionUBO,
}

/// The size of the texture `objects` binds at `slot` in `context`,
/// zero for none.
fn texture_size(context: &Context, objects: &DrawObjects, slot: usize) -> [f32; 2] {
    objects.texture(slot).and_then(|texture| context.texture(texture)).map_or([0.0, 0.0], |texture| {
        let size = texture.size();
        [size.width as f32, size.height as f32]
    })
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into the draws of a tile, `draws`, all of which read the same
/// attributes (a halo and a fill): their triangles, and the attributes,
/// which the first of them that is visible copies and the others then read.
fn refresh_draws(
    context: &mut Context,
    draws: &mut [SymbolDraw],
    geometry: &BucketGeometry,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
) {
    let mut source = None;
    for (index, draw) in draws.iter_mut().enumerate() {
        if refresh_draw(context, draw, geometry, bucket, layer_id, evaluated, refresh, source.is_none()) {
            source = Some(index);
        }
    }
    let Some(source) = source else { return };
    let (before, rest) = draws.split_at_mut(source);
    let Some((source, after)) = rest.split_first_mut() else { return };
    for draw in before.iter_mut().chain(after) {
        draw.objects.share_vertex_attributes(&source.objects);
        draw.placed = source.placed.clone();
    }
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into a draw of it: its triangles, and where `copy_attributes`, its
/// attributes. Returns whether it copied the attributes.
#[expect(clippy::too_many_arguments, reason = "the draw and its tile's bucket, what changed of it, and the context")]
fn refresh_draw(
    context: &mut Context,
    draw: &mut SymbolDraw,
    geometry: &BucketGeometry,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
    copy_attributes: bool,
) -> bool {
    let is_text = draw.data.symbol_type == SymbolType::Text;
    let sdf_icons = draw.data.symbol_type == SymbolType::IconSDF;

    let state = &bucket.state;

    // This property can be set after the initial appearance of the tile, as
    // part of the layout process.
    draw.data.bucket_variable_placement = state.has_variable_placement;

    let buffer = if is_text {
        &state.text
    } else if sdf_icons {
        &state.sdf_icon
    } else {
        &state.icon
    };

    let was_visible = draw.visible;
    if refresh.opacity || refresh.sort {
        draw.visible = buffer.segment_has_visible_vertices(draw.data.segment);
    }
    if !draw.visible {
        return false;
    }
    let refresh = if was_visible { refresh } else { BucketChanges::ALL };

    if !refresh.any() {
        return false;
    }

    if copy_attributes {
        let attrs = draw.objects.vertex_attributes_mut();
        if refresh.dynamic {
            set_projected_pos_attribute(context, buffer, layer_id, attrs, &mut draw.placed.projected);
        }
        if refresh.opacity {
            set_fade_opacity_attribute(context, buffer, attrs, &mut draw.placed.opacity);
            set_elevation_attribute(context, geometry, buffer, attrs, &mut draw.placed.elevation);
        }
        if refresh.paint
            && let Some(paint_props) = bucket.paint_properties.get(layer_id)
        {
            set_binder_attributes(context, geometry, is_text, paint_props, evaluated, attrs, &mut Uniforms::default());
        }
    }

    if refresh.sort && !buffer.triangles.is_empty() {
        let segments = match draw.data.segment {
            Some(i) => buffer.segments.get(i..=i).unwrap_or_default(),
            None => &buffer.segments,
        };
        let segments = segments.iter().map(SegmentBase::copy).collect();
        draw.objects.replace_indexes(copied_indexes(&buffer.triangles), segments);
    }
    copy_attributes
}

/// One segment, or all the segments, of a buffer of a tile's bucket, which
/// the update builds draws of in the order of [`SegmentGroup::less`].
struct SegmentGroup {
    sort_key: f32,
    type_: SymbolType,
    /// The tile's index among the layer's tiles of the frame.
    tile_index: usize,
    tile_id: UnwrappedTileID,
    overscaled_z: u8,
    /// One segment, for a layer that sorts its features by key, or all
    /// segments of the buffer.
    segment: Option<usize>,
}

impl SegmentGroup {
    fn less(lhs: &SegmentGroup, rhs: &SegmentGroup) -> bool {
        // Sort renderable segments by a sort key.
        if lhs.sort_key < rhs.sort_key {
            return true;
        }

        // In cases when sort key is the same, sort by the type of a segment
        // (text over icons), and for segments of the same type with the same
        // sort key, sort by a tile id.
        if lhs.sort_key == rhs.sort_key {
            if lhs.type_ != SymbolType::Text && rhs.type_ == SymbolType::Text {
                return true;
            }

            if lhs.type_ == rhs.type_ {
                return lhs.tile_id < rhs.tile_id;
            }
        }

        false
    }
}

impl LayerType for Symbol {
    type Layout = SymbolLayout;
    type Bucket = SymbolBucket;
    type TileBucket<'a> = LayerBucket<'a, Symbol>;

    /// A symbol layout always waits for its glyphs and images.
    fn layout_can_need_dependencies(_group: &[Arc<EvaluatedLayer<Symbol>>]) -> bool {
        true
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Symbol>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<SymbolLayout>, Error> {
        SymbolLayout::new(parameters.bucket_parameters, group, source_layer, parameters).map(Some)
    }

    fn render_layer(style: Arc<LayerStyle<Symbol>>) -> Box<dyn RenderLayer> {
        Box::new(RenderSymbolLayer::new(style))
    }
}

pub(crate) struct RenderSymbolLayer {
    base: RenderLayerBase,
    values: LayerValues<Symbol>,
    drawing: SymbolDrawing,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<SymbolUniforms, usize>,
    /// The segments of the tiles the update builds draws of, as it sorts
    /// them, kept from frame to frame for their memory.
    renderable_segments: Vec<SegmentGroup>,
    /// The buckets the layer places, in placement order.
    placement_data: Vec<BucketPlacementData>,
    /// The buckets another layer places and this layer draws.
    follower_data: Vec<BucketPlacementData>,
    /// The layer's source, which its buckets' placement data shares.
    source_id: Arc<str>,

    icon_size: f32,
    text_size: f32,

    /// The passes the layer's draws were built for.
    draws_passes: RenderPass,
    /// The indexes of the buckets `HeldBuckets` holds for the layer.
    #[cfg(feature = "bench")]
    held: Vec<usize>,
}

#[cfg(feature = "bench")]
impl Drop for RenderSymbolLayer {
    fn drop(&mut self) {
        for &index in &self.held {
            crate::render_layer::HeldBuckets::release(index);
        }
    }
}

impl RenderSymbolLayer {
    /// The translations of the layer's text and of its icons, as its
    /// evaluated paint properties set them.
    pub(crate) fn symbol_translates(&self) -> (SymbolTranslate, SymbolTranslate) {
        let evaluated = &self.values.evaluated.paint;
        let text_translate = SymbolTranslate {
            offset: *evaluated.text_translate.uniform(),
            anchor: *evaluated.text_translate_anchor.uniform(),
        };
        let icon_translate = SymbolTranslate {
            offset: *evaluated.icon_translate.uniform(),
            anchor: *evaluated.icon_translate_anchor.uniform(),
        };
        (text_translate, icon_translate)
    }

    /// Adds `bucket`, `key`'s, of `tile`, to what the layer places: when the
    /// layer leads the bucket, to its placement data, with the tile's feature
    /// index, in order of sort key; otherwise to its follower data.
    pub(crate) fn add_bucket(
        &mut self,
        tile: &PlacedTile,
        key: BucketKey,
        bucket: &SymbolBucket,
        feature_index: impl FnOnce() -> Option<Arc<FeatureIndex>>,
        (text_translate, icon_translate): (SymbolTranslate, SymbolTranslate),
    ) {
        let source = &self.base.style.base().source;
        if *self.source_id != **source {
            self.source_id = Arc::from(source.as_str());
        }
        let source_id = &self.source_id;
        let data = |feature_index, sort_key_range| BucketPlacementData {
            bucket: key,
            tile: *tile,
            feature_index,
            source_id: Arc::clone(source_id),
            sort_key_range,
            text_translate,
            icon_translate,
        };
        if *bucket.bucket_leader_id != *self.base.style.base().id {
            let data = data(None, None);
            self.follower_data.push(data);
            return;
        }
        // Only place this layer if it's the "group leader" for the bucket
        let feature_index = feature_index();

        if bucket.sort_key_ranges.is_empty() {
            let data = data(feature_index, None);
            self.placement_data.push(data);
            return;
        }
        for sort_key_range in &bucket.sort_key_ranges {
            let layer_data = data(feature_index.clone(), Some(*sort_key_range));
            if self
                .placement_data
                .last()
                .is_none_or(|last| last.sort_key_range.is_some_and(|last| last.sort_key <= sort_key_range.sort_key))
            {
                // Already in order, as ranges with a constant sort key always are.
                self.placement_data.push(layer_data);
                continue;
            }
            // After every range of a sort key no greater, as ranges of
            // equal keys keep their order.
            let sort_position = self.placement_data.partition_point(|lhs| {
                let key = lhs.sort_key_range.expect("sort key range").sort_key;
                sort_key_range.sort_key.partial_cmp(&key) != Some(std::cmp::Ordering::Less)
            });
            self.placement_data.insert(sort_position, layer_data);
        }
    }

    /// [`add_bucket`](Self::add_bucket) of the bucket that
    /// `HeldBuckets` holds at `bucket` ([`crate::bucket::recover`]), of the
    /// tile at `tile_index` of `tiles`. The layer frees the bucket when it
    /// drops.
    #[cfg(feature = "bench")]
    pub(crate) fn add_placement_data(
        &mut self,
        tiles: &[RenderTile],
        tile_index: usize,
        bucket: usize,
        feature_index: impl FnOnce() -> Option<Arc<FeatureIndex>>,
        translates: (SymbolTranslate, SymbolTranslate),
    ) {
        use crate::render_layer::{HeldBuckets, SymbolBuckets as _};
        self.held.push(bucket);
        let Some(tile) = tiles.get(tile_index) else {
            return;
        };
        let key = BucketKey {
            bucket,
            slot: tile.slot(),
            tile: *tile.overscaled_tile_id(),
            source: crate::tile::job::SourceKey(0),
        };
        let held = HeldBuckets::take();
        if let Some(bucket) = held.get(&key) {
            self.add_bucket(&PlacedTile::new(tile), key, bucket, feature_index, translates);
        }
        held.restore();
    }

    /// Whether the layer places or draws any bucket.
    #[cfg(feature = "bench")]
    pub(crate) fn has_placement_data(&self) -> bool {
        !self.placement_data.is_empty() || !self.follower_data.is_empty()
    }

    pub(crate) fn new(impl_: Arc<LayerStyle<Symbol>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderSymbolLayer {
            base,
            values,
            drawing: SymbolDrawing::default(),
            draw_list: DrawList::default(),
            renderable_segments: Vec::new(),
            placement_data: Vec::new(),
            follower_data: Vec::new(),
            source_id: Arc::from(""),
            icon_size: 1.0,
            text_size: 16.0,
            draws_passes: RenderPass::None,
            #[cfg(feature = "bench")]
            held: Vec::new(),
        }
    }

    /// The entries in the layer's storage buffers of `draw`, of the tile
    /// `tile_id` and its `bucket`, of the layer `layer_id` with the values
    /// `layer`.
    fn uniforms(
        layer_id: &str,
        layer: &EvaluatedLayer<Symbol>,
        tile_id: &OverscaledTileID,
        bucket: &SymbolBucket,
        draw: &SymbolDraw,
        parameters: &PaintParameters<'_>,
    ) -> Option<SymbolUniforms> {
        let state = parameters.view;
        let evaluated = &layer.paint;
        let zoom = state.zoom() as f32;
        let paint_properties = bucket.paint_properties.get(layer_id)?;

        let data = &draw.data;
        let tile_id = tile_id.to_unwrapped();
        let is_text = data.symbol_type == SymbolType::Text;
        let screen_space_prop = &layer.style.layout.symbol_screen_space;
        let is_screen_space = screen_space_prop
            .as_constant()
            .copied()
            .unwrap_or_else(SymbolLayoutProperties::Defaults::symbol_screen_space);

        let translate = if is_text { &evaluated.text_translate } else { &evaluated.icon_translate };
        let anchor = if is_text { &evaluated.text_translate_anchor } else { &evaluated.icon_translate_anchor };
        let mut projection: ProjectionData = if is_screen_space {
            Projection::Mercator.projection_data(state, &tile_id, &screen_matrix(*translate.uniform()))
        } else {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false;
            projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                *anchor.uniform(),
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                None,
                false,
            )
        };
        // The translation stays out of the matrices on every projection: the
        // shader adds it to point anchors, and placement to the positions it
        // lays out (variable anchors, line labels), so it applies once.
        let tile_translation: [f32; 2] = if is_screen_space {
            [0.0, 0.0]
        } else {
            RenderTile::tile_unit_translation(&tile_id, *translate.uniform(), *anchor.uniform(), state)
        };
        projection.translate = glam::DVec2::ZERO;
        let matrix = projection.fallback_matrix;

        let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
        let pitch_with_map = data.pitch_alignment == AlignmentType::Map;
        let rotate_with_map = data.rotation_alignment == AlignmentType::Map;
        let along_line = data.along_line;
        let has_variable_placement =
            data.bucket_variable_placement && (is_text || data.text_fit != IconTextFitType::None);
        let label_plane_matrix = if along_line || has_variable_placement {
            DMat4::IDENTITY
        } else {
            label_plane_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units)
        };
        let gl_coord_matrix = gl_coord_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);
        let gamma_scale =
            if pitch_with_map { (state.pitch().cos() * state.camera_to_center_distance()) as f32 } else { 1.0 };

        // Line labels rotate as their placement projects them along their
        // lines (`reproject_line_labels`). Pitched point labels rotate with
        // the label plane matrix's projection. Unpitched point labels need
        // their rotation applied after projection.
        let rotate_in_shader = rotate_with_map && !pitch_with_map && !along_line;

        let size_binder = if is_text { &bucket.text_size_binder } else { &bucket.icon_size_binder };
        let size = size_binder.evaluate_for_zoom(zoom);

        let (fill_color_t, halo_color_t, opacity_t, halo_width_t, halo_blur_t) = if is_text {
            let b = &paint_properties.text_binders;
            (
                b.text_color.interpolation_factor(zoom),
                b.text_halo_color.interpolation_factor(zoom),
                b.text_opacity.interpolation_factor(zoom),
                b.text_halo_width.interpolation_factor(zoom),
                b.text_halo_blur.interpolation_factor(zoom),
            )
        } else {
            let b = &paint_properties.icon_binders;
            (
                b.icon_color.interpolation_factor(zoom),
                b.icon_halo_color.interpolation_factor(zoom),
                b.icon_opacity.interpolation_factor(zoom),
                b.icon_halo_width.interpolation_factor(zoom),
                b.icon_halo_blur.interpolation_factor(zoom),
            )
        };

        Some(SymbolUniforms {
            draw_ubo: SymbolDrawableUBO {
                matrix: cast_matrix(&matrix),
                label_plane_matrix: cast_matrix(&label_plane_matrix),
                coord_matrix: cast_matrix(&gl_coord_matrix),
                texsize: texture_size(parameters.context, &draw.objects, TEXTURE_SLOT),
                texsize_icon: texture_size(parameters.context, &draw.objects, TEXTURE_ICON_SLOT),
                is_text_prop: i32::from(is_text),
                rotate_symbol: i32::from(rotate_in_shader),
                pitch_with_map: i32::from(pitch_with_map),
                is_size_zoom_constant: i32::from(size.kind.is_zoom_constant()),
                is_size_feature_constant: i32::from(size.kind.is_feature_constant()),
                is_offset: i32::from(data.is_offset),
                size_t: size.size_t,
                size: size.size,
                fill_color_t,
                halo_color_t,
                opacity_t,
                halo_width_t,
                halo_blur_t,
                is_along_line: i32::from(along_line),
                is_variable_anchor: i32::from(has_variable_placement),
                pitched_scale: state.projection().circle_radius_correction(state) as f32,
                translation: tile_translation,
                gamma_scale,
                is_halo: i32::from(data.is_halo),
            },
            tile_props: SymbolTilePropsUBO {
                is_text: i32::from(is_text),
                is_halo: i32::from(data.is_halo),
                pad1: 0.0,
                pad2: 0.0,
            },
            projection: to_projection_ubo(&projection),
        })
    }

    /// Makes the frame's draws of the layer's draws of `tiles` that show any
    /// symbols, in `passes`, which hide behind the ground in front of them
    /// where they draw with a variant that stands on terrain (`stands`), and
    /// writes the buffers they read.
    fn make_frame_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        tiles: &[FrameTile<'_, Symbol>],
        passes: RenderPass,
        stands: bool,
        paint_parameters: &mut PaintParameters<'_>,
    ) {
        let layer_id = self.base.style.base().id.as_str();
        let layer = &*self.values.evaluated;
        let ground = paint_parameters.ground.clone().filter(|_| stands);
        let depth = ground.as_deref().and_then(FrameGround::depth);
        for tile in tiles {
            let tile_id = tile.id();
            let Some(cached) = self.drawing.tiles.get_mut(&tile_id) else {
                continue;
            };
            let bucket = tile.bucket().map(|bucket| bucket.bucket);
            debug_assert!(bucket.is_some() || cached.draws.is_empty());
            for (index, draw) in cached.draws.iter_mut().enumerate().filter(|(_, draw)| draw.visible) {
                let Some(uniforms) =
                    bucket.and_then(|bucket| Self::uniforms(layer_id, layer, &tile_id, bucket, draw, paint_parameters))
                else {
                    continue;
                };
                if let Some(depth) = depth {
                    draw.objects.set_texture(GROUND_DEPTH_SLOT, Some(depth));
                }
                let frame_draw = FrameDraw { pass: passes, ..FrameDraw::new(Some(tile_id), draw.id) };
                self.draw_list.push_part(frame_draw, index, uniforms);
            }
        }

        if let Some(frame) = &mut self.drawing.frame {
            if self
                .draw_list
                .finish(frame, gpu, paint_parameters.context, |writer| write_uniforms(&layer.paint, writer))
            {
                for draw in self.drawing.tiles.values_mut().flat_map(|tile| &mut tile.draws) {
                    draw.objects.drop_bindings();
                }
            }
        } else {
            self.draw_list.clear();
        }
    }
}

/// Writes the buffers of the frame's draws of a layer of the paint values
/// `evaluated`.
fn write_uniforms(
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    writer: &mut UniformWriter<'_, SymbolUniforms>,
) {
    let props = SymbolEvaluatedPropsUBO {
        text_fill_color: evaluated.text_color.uniform().to_float_array(),
        text_halo_color: evaluated.text_halo_color.uniform().to_float_array(),
        text_opacity: *evaluated.text_opacity.uniform(),
        text_halo_width: *evaluated.text_halo_width.uniform(),
        text_halo_blur: *evaluated.text_halo_blur.uniform(),
        pad1: 0.0,
        icon_fill_color: evaluated.icon_color.uniform().to_float_array(),
        icon_halo_color: evaluated.icon_halo_color.uniform().to_float_array(),
        icon_opacity: *evaluated.icon_opacity.uniform(),
        icon_halo_width: *evaluated.icon_halo_width.uniform(),
        icon_halo_blur: *evaluated.icon_halo_blur.uniform(),
        pad2: 0.0,
    };
    writer.uniform(PROPS_ID, &props);
    writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
    writer.storage(TILE_PROPS_ID, |uniforms| uniforms.tile_props);
    writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
}

impl TypedLayer for RenderSymbolLayer {
    type Type = Symbol;
    type Drawing = SymbolDrawing;

    /// Symbols draw the lower of two tiles' overlapping symbols over the
    /// higher's.
    const TILE_ORDER: TileOrder = TileOrder::ByY;

    /// Over terrain, the symbols stand on the ground under their anchors,
    /// which placement gives them from the surface the frame drew, and hide
    /// behind the ground in front.
    const STANDS: bool = true;
    const HIDES_BEHIND_GROUND: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// The text of a `text-field` with format sections takes the values
    /// its sections override.
    fn complete_evaluation(
        &mut self,
        parameters: &PropertyEvaluationParameters,
        evaluated: &mut EvaluatedLayer<Symbol>,
    ) {
        if SymbolLayerPaintPropertyOverrides::has_overrides(&self.values.style.layout.text_field) {
            mln_style::layers::symbol_layer::set_overrides(
                &self.values.style.layout,
                &mut evaluated.paint,
                &self.values.unevaluated,
                &parameters.global_state,
            );
        }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Symbol>) -> RenderPass {
        let evaluated = &evaluated.paint;
        let has_icon_opacity = evaluated.icon_color.constant_or(&SymbolPaintProperties::Defaults::icon_color()).a > 0.0
            || evaluated.icon_halo_color.constant_or(&SymbolPaintProperties::Defaults::icon_halo_color()).a > 0.0;
        let has_text_opacity = evaluated.text_color.constant_or(&SymbolPaintProperties::Defaults::text_color()).a > 0.0
            || evaluated.text_halo_color.constant_or(&SymbolPaintProperties::Defaults::text_halo_color()).a > 0.0;

        if (evaluated.icon_opacity.constant_or(&1.0) > 0.0 && has_icon_opacity && self.icon_size > 0.0)
            || (evaluated.text_opacity.constant_or(&1.0) > 0.0 && has_text_opacity && self.text_size > 0.0)
        {
            RenderPass::Translucent
        } else {
            RenderPass::None
        }
    }

    /// Makes the frame's draws of the layer: those of each of its tiles,
    /// which it builds for a tile it keeps none of and refreshes where the
    /// tile's bucket changed, and writes the buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Symbol>],
    ) {
        /// The atlases' textures given to one tile's draws.
        #[derive(Default)]
        struct TileAtlases {
            text: Option<AtlasBinding>,
            icon: Option<AtlasBinding>,
        }

        let transform = paint_parameters.view;
        let passes = self.base.passes;
        if passes != self.draws_passes {
            self.drawing.clear();
            self.draws_passes = passes;
        }

        let layout = &self.values.style.layout;
        let sort_features_by_key = layout.symbol_sort_key.is_expression();
        let constant_sort_key = layout.symbol_sort_key.is_constant();
        // Screen-space symbols are drawn over the view, not projected onto
        // the map.
        let screen_space_prop = &layout.symbol_screen_space;
        let is_screen_space = screen_space_prop
            .as_constant()
            .copied()
            .unwrap_or_else(SymbolLayoutProperties::Defaults::symbol_screen_space);
        let symbol_projection_variant =
            if is_screen_space { ProjectionVariant::Mercator } else { self.base.projection_variant };
        let is_offset = !layout.icon_offset.is_undefined();

        let current_zoom = transform.zoom() as f32;

        let mut renderable_segments = std::mem::take(&mut self.renderable_segments);
        renderable_segments.clear();

        for (tile_index, tile) in render_tiles.iter().enumerate() {
            let tile_id = tile.id();

            let Some(mut entry) = self.drawing.tiles.entry(&tile_id) else {
                continue;
            };
            let render_data = tile.bucket_for_pass(passes);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;
            let layer_id = render_data.layer.style.base().id.as_str();

            // Draws built from another bucket go.
            entry.set_bucket(bucket.base.bucket_id);
            // What changed since the draws were built or last refreshed.
            let refresh = entry.take_changes();

            debug_assert!(bucket.paint_properties.contains_key(layer_id));
            if !bucket.paint_properties.contains_key(layer_id) {
                continue;
            }

            let evaluated = &render_data.layer.paint;

            // A tile the layer keeps draws of refreshes them.
            if let Some(cached) = entry.value.as_mut().filter(|cached| !cached.draws.is_empty()) {
                // The draws that read the same attributes are next to each
                // other, as the update builds them.
                let (context, geometry) = (&mut *paint_parameters.context, render_data.geometry());
                for draws in cached.draws.chunk_by_mut(|a, b| a.attributes_key() == b.attributes_key()) {
                    refresh_draws(context, draws, geometry, bucket, layer_id, evaluated, refresh);
                }
                continue;
            }

            let mut serial_key = 1.0_f32;
            let state = &bucket.state;
            // A bucket laid out before a layout change can still have the
            // previous sort-key expression until it is rebuilt.
            let sort_segments =
                sort_features_by_key || (constant_sort_key && !bucket.layout.symbol_sort_key.is_constant());
            let mut add_renderables = |buffer: &Buffer, type_: SymbolType| {
                if sort_segments {
                    // Features need to be rendered in a specific order, so we
                    // add each segment individually
                    for (i, segment) in buffer.segments.iter().enumerate() {
                        renderable_segments.push(SegmentGroup {
                            sort_key: segment.sort_key,
                            type_,
                            tile_index,
                            tile_id: tile.tile.id,
                            overscaled_z: tile_id.overscaled_z,
                            segment: Some(i),
                        });
                    }
                } else if !buffer.segments.is_empty() {
                    // Features can be rendered in the order produced, and as
                    // grouped by the bucket. With an explicit constant key,
                    // the segment's key keeps text above icons across tiles.
                    let sort_key = if constant_sort_key { buffer.segments[0].sort_key } else { serial_key };
                    renderable_segments.push(SegmentGroup {
                        sort_key,
                        type_,
                        tile_index,
                        tile_id: tile.tile.id,
                        overscaled_z: tile_id.overscaled_z,
                        segment: None,
                    });
                    serial_key += 1.0;
                }
            };

            let atlases = render_data.atlas_textures();
            let has_icon_texture = atlases.icon().is_some();
            let has_glyph_texture = atlases.glyph().is_some();

            if !state.icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state.icon, SymbolType::IconRGBA);
            }
            if !state.sdf_icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state.sdf_icon, SymbolType::IconSDF);
            }
            if !state.text.segments.is_empty() && has_glyph_texture {
                add_renderables(&state.text, SymbolType::Text);
            }
        }

        // A stable sort keeps segments of the same order in the order they
        // were added.
        renderable_segments.sort_by(|a, b| {
            if SegmentGroup::less(a, b) {
                std::cmp::Ordering::Less
            } else if SegmentGroup::less(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        // Renderables come across tiles, out of order, so the atlases' textures
        // given to a tile's draws are kept by tile.
        let mut tile_atlases: HashMap<UnwrappedTileID, TileAtlases> = HashMap::default();
        let color_mode = if (passes.underlying_type() & RenderPass::Translucent.underlying_type()) != 0 {
            ColorMode::alpha_blended()
        } else {
            ColorMode::unblended()
        };

        for group in &renderable_segments {
            let is_text = group.type_ == SymbolType::Text;
            let sdf_icons = group.type_ == SymbolType::IconSDF;

            let Some(tile) = render_tiles.get(group.tile_index) else {
                continue;
            };
            let tile_id = tile.tile.id.overscale_to(group.overscaled_z);
            let Some(render_data) = tile.bucket_for_pass(passes) else {
                continue;
            };
            let bucket = render_data.bucket;
            let state = &bucket.state;
            let buffer = if is_text {
                &state.text
            } else if sdf_icons {
                &state.sdf_icon
            } else {
                &state.icon
            };

            if buffer.triangles.is_empty() {
                continue;
            }
            let visible = buffer.segment_has_visible_vertices(group.segment);

            let evaluated = &render_data.layer.paint;
            let layer_id = render_data.layer.style.base().id.as_str();
            let Some(bucket_paint_properties) = bucket.paint_properties.get(layer_id) else {
                continue;
            };

            let bucket_layout = &*bucket.layout;
            let values =
                if is_text { text_property_values(bucket_layout) } else { icon_property_values(bucket_layout) };

            let atlases = render_data.atlas_textures();

            let mut uniforms = Uniforms::default();
            let mut vertex_attribs = VertexAttributeArray::new();
            let placed = update_tile_attributes(
                paint_parameters.context,
                render_data.geometry(),
                buffer,
                layer_id,
                is_text,
                bucket_paint_properties,
                evaluated,
                &mut vertex_attribs,
                &mut uniforms,
            );

            let text_halo = evaluated.text_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.text_halo_width.constant_or(&1.0) != 0.0;
            let text_fill = evaluated.text_color.constant_or(&Color::black()).a > 0.0;

            let icon_halo = evaluated.icon_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.icon_halo_width.constant_or(&1.0) != 0.0;
            let icon_fill = evaluated.icon_color.constant_or(&Color::black()).a > 0.0;

            // The programs, halo first, with whether each is a halo's and
            // draws.
            let halo_and_fill = if is_text {
                let program =
                    if bucket.icons_in_text { &programs::SYMBOL_TEXT_AND_ICON } else { &programs::SYMBOL_SDF };
                vec![(program, true, text_halo), (program, false, text_fill)]
            } else if sdf_icons {
                vec![(&programs::SYMBOL_SDF, true, icon_halo), (&programs::SYMBOL_SDF, false, icon_fill)]
            } else {
                vec![(&programs::SYMBOL_ICON, false, true)]
            };

            let segments = match group.segment {
                Some(i) => std::slice::from_ref(&buffer.segments[i]),
                None => &buffer.segments,
            };
            for (program, is_halo, _) in halo_and_fill.into_iter().filter(|(_, _, drawn)| *drawn) {
                // The draws of a tile share the atlases' textures.
                let tile_atlases = tile_atlases.entry(tile.tile.id).or_default();
                let draw_atlases = if is_text {
                    tile_atlases.text.get_or_insert_with(|| {
                        let text_size_is_zoom_constant =
                            bucket.text_size_binder.evaluate_for_zoom(current_zoom).kind.is_zoom_constant();
                        AtlasBinding::new(
                            Some(atlases.clone()),
                            Some(TEXTURE_ICON_SLOT),
                            Some(TEXTURE_SLOT),
                            is_text,
                            false,
                            values.rotation_alignment,
                            false,
                            text_size_is_zoom_constant,
                        )
                    })
                } else {
                    tile_atlases.icon.get_or_insert_with(|| {
                        let icon_scaled = bucket_layout.icon_size.constant_or(&1.0) != 1.0 || bucket.icons_need_linear;
                        AtlasBinding::new(
                            Some(atlases.clone()),
                            Some(TEXTURE_ICON_SLOT),
                            Some(TEXTURE_SLOT),
                            is_text,
                            sdf_icons,
                            values.rotation_alignment,
                            icon_scaled,
                            false,
                        )
                    })
                };

                // The halo and the fill read the same attributes.
                let geometry = DrawGeometry {
                    vertices: copy_shared_attributes(&vertex_attribs),
                    vertex_count: buffer.attribute_data.len(),
                    mode: DrawMode::Triangles(),
                    indexes: copied_indexes(&buffer.triangles),
                    segments: segments.iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                let shader = paint_parameters.context.shader_key(program, uniforms, symbol_projection_variant);
                let textures = draw_atlases.textures(&shader);
                let objects = DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), geometry, textures);

                let draw = SymbolDraw {
                    id: SimpleIdentity::new(),
                    objects,
                    placed: placed.clone(),
                    data: SymbolDrawData {
                        is_halo,
                        bucket_variable_placement: state.has_variable_placement,
                        symbol_type: group.type_,
                        pitch_alignment: values.pitch_alignment,
                        rotation_alignment: values.rotation_alignment,
                        along_line: values.along_line,
                        text_fit: *bucket_layout.icon_text_fit.uniform(),
                        is_offset,
                        segment: group.segment,
                    },
                    visible,
                    atlases: draw_atlases.clone(),
                };
                if let Some(entry) = self.drawing.tiles.entry(&tile_id) {
                    entry.value.get_or_insert_with(SymbolTile::default).draws.push(draw);
                }
            }
        }

        self.renderable_segments = renderable_segments;
        self.make_frame_draws(gpu, render_tiles, passes, symbol_projection_variant.stands(), paint_parameters);
    }

    /// Collects the symbol buckets of the tiles the layer places, and of
    /// those whose buckets another layer places and this one draws.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: &[FrameTile<'_, Symbol>]) {
        self.placement_data.clear();
        self.follower_data.clear();
        let Some(source) = params.source.and_then(RenderSource::tiles) else {
            return;
        };
        let translates = self.symbol_translates();
        for tile in tiles {
            let Some(bucket) = tile.bucket() else {
                continue;
            };
            let key = BucketKey { source, tile: tile.id(), slot: tile.tile.slot(), bucket: bucket.bucket_index };
            let feature_index = || Some(Arc::clone(bucket.feature_index()));
            self.add_bucket(&PlacedTile::new(tile.tile), key, bucket.bucket, feature_index, translates);
        }
    }

    fn release(&mut self) {
        self.placement_data.clear();
    }
}

impl RenderLayer for RenderSymbolLayer {
    fn placement_data(&self) -> &[BucketPlacementData] {
        &self.placement_data
    }

    fn follower_data(&self) -> &[BucketPlacementData] {
        &self.follower_data
    }
}
