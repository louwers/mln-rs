// A tile's draws get copies of the bucket's vertex vectors. When placement,
// sorting or feature state change the bucket (the changes the tile's entry
// in the layer's `TileMap` collects), `update` replaces just those
// attributes (opacity, dynamic positions, binders) and the triangle indices
// of the tile's draws.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_DRAWABLE_UBO, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_PROJECTION_UBO, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_COLLISION_TILE_PROPS_UBO, ID_PROJECTION_UBO, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_DRAWABLE_UBO,
    ID_SYMBOL_EVALUATED_PROPS_UBO, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, ID_SYMBOL_TILE_PROPS_UBO,
};
use mln_gfx::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::vertex_vector::VertexVector;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_map_types::{Projection, ProjectionData};
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Symbol;
use mln_style::layers::symbol_layer::SymbolLayerPaintPropertyOverrides;
use mln_style::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties};
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_style_types::{
    AlignmentType, IconTextFitType, SymbolPlacementType, TextRotationAlignmentType, TranslateAnchorType,
};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::{Mat4, matrix};

use super::{cast_matrix, shared_vertices};
use crate::bucket::Bucket;
use crate::bucket_geometry::{BucketGeometry, bucket_vertices, paint_vertices, released_indexes};
use crate::buckets::symbol_bucket::{
    Buffer, CollisionBoxDynamicVertex, CollisionBoxLayoutVertex, CollisionBuffer, PaintProperties, SymbolBucket,
};
use crate::draw::{DrawGeometry, DrawObjects, Textures, copy_shared_attributes};
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::layer_drawing::{
    BucketChanges, DrawDepth, DrawGroup, DrawList, DrawObjectsMut, DrawOrder, FrameDraw, KeptDraw, LayerDrawing,
    LayerDraws, TileMap, UniformWriter,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::symbol_layout::SymbolLayout;
use crate::layout::symbol_projection::{get_gl_coord_matrix, get_label_plane_matrix};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{Atlases, 
    BucketKey, BucketPlacementData, LayerParts, LayerSource, LayerTiles, LayerUpdateParameters, LayerValues, Parts,
    PartsMut, PlacedTile, RenderLayer, RenderLayerBase, SymbolTranslate, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_source::TileOrder;
use crate::render_tile::{FrameTile, RenderTile};
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;

const SYMBOL_ICON_SHADER_NAME: &str = "SymbolIconShader";
const SYMBOL_SDF_SHADER_NAME: &str = "SymbolSDFShader";
const SYMBOL_TEXT_AND_ICON_SHADER_NAME: &str = "SymbolTextAndIconShader";

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
    pub(crate) translate: [f32; 2],
    pub(crate) translate_anchor: TranslateAnchorType,
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
    /// The bucket segment the draw draws (`sortFeaturesByKey`), or all of
    /// them.
    segment: Option<usize>,
}

#[must_use]
pub(crate) fn icon_property_values(
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    layout: &SymbolLayoutProperties::PossiblyEvaluated,
) -> SymbolPropertyValues {
    let rotation_alignment = *layout.icon_rotation_alignment.uniform();
    SymbolPropertyValues {
        pitch_alignment: *layout.icon_pitch_alignment.uniform(),
        rotation_alignment,
        along_line: rotation_alignment == AlignmentType::Map
            && *layout.symbol_placement.uniform() != SymbolPlacementType::Point,
        translate: *evaluated.icon_translate.uniform(),
        translate_anchor: *evaluated.icon_translate_anchor.uniform(),
    }
}

#[must_use]
pub(crate) fn text_property_values(
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    layout: &SymbolLayoutProperties::PossiblyEvaluated,
) -> SymbolPropertyValues {
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
        translate: *evaluated.text_translate.uniform(),
        translate_anchor: *evaluated.text_translate_anchor.uniform(),
    }
}

/// The tile matrix of screen-space symbols.
pub(crate) fn get_screen_matrix(translation: [f32; 2]) -> Mat4 {
    let mut m: Mat4 = [0.0; 16];
    matrix::ortho(&mut m, 0.0, f64::from(EXTENT), -f64::from(EXTENT), 0.0, 0.0, 1.0);
    let a = m;
    matrix::translate(&mut m, &a, 0.0, -f64::from(EXTENT), 0.0);
    let a = m;
    matrix::translate(&mut m, &a, f64::from(translation[0]), f64::from(translation[1]), 0.0);
    m
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bucket's buffers and the layer's paint properties, and the attributes and uniforms they fill"
)]
fn update_tile_attributes(
    geometry: Option<&BucketGeometry>,
    buffer: &Buffer,
    layer_id: &str,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    set_binder_attributes(geometry, is_text, paint_props, evaluated, attribs, properties_as_uniforms);
    set_static_and_dynamic_attributes(geometry, buffer, layer_id, attribs);
}

/// The data-driven paint property half of `updateTileAttributes`.
fn set_binder_attributes(
    geometry: Option<&BucketGeometry>,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    let mut id = ID_SYMBOL_OPACITY_ATTRIBUTE;
    let pau = properties_as_uniforms;
    if is_text {
        let b = &paint_props.text_binders;
        let ib = &b.interleaved_vertex_buffer;
        let shared = paint_vertices(geometry, ib, b.depends_on_feature_state());
        read_data_driven_paint_property(
            attribs,
            &b.text_opacity,
            ib,
            &shared,
            &["a_opacity"],
            evaluated.text_opacity.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_color,
            ib,
            &shared,
            &["a_fill_color"],
            evaluated.text_color.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_color,
            ib,
            &shared,
            &["a_halo_color"],
            evaluated.text_halo_color.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_width,
            ib,
            &shared,
            &["a_halo_width"],
            evaluated.text_halo_width.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.text_halo_blur,
            ib,
            &shared,
            &["a_halo_blur"],
            evaluated.text_halo_blur.is_constant(),
            pau,
            &mut id,
        );
    } else {
        let b = &paint_props.icon_binders;
        let ib = &b.interleaved_vertex_buffer;
        let shared = paint_vertices(geometry, ib, b.depends_on_feature_state());
        read_data_driven_paint_property(
            attribs,
            &b.icon_opacity,
            ib,
            &shared,
            &["a_opacity"],
            evaluated.icon_opacity.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_color,
            ib,
            &shared,
            &["a_fill_color"],
            evaluated.icon_color.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_color,
            ib,
            &shared,
            &["a_halo_color"],
            evaluated.icon_halo_color.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_width,
            ib,
            &shared,
            &["a_halo_width"],
            evaluated.icon_halo_width.is_constant(),
            pau,
            &mut id,
        );
        read_data_driven_paint_property(
            attribs,
            &b.icon_halo_blur,
            ib,
            &shared,
            &["a_halo_blur"],
            evaluated.icon_halo_blur.is_constant(),
            pau,
            &mut id,
        );
    }
}

/// The vertex-buffer half of `updateTileAttributes`.
fn set_static_and_dynamic_attributes(
    geometry: Option<&BucketGeometry>,
    buffer: &Buffer,
    layer_id: &str,
    attribs: &mut VertexAttributeArray,
) {
    set_layout_attributes(geometry, buffer, attribs);
    set_projected_pos_attribute(buffer, layer_id, attribs);
    set_fade_opacity_attribute(buffer, attribs);
}

/// The static half of `updateTileAttributes`: the layout vertices.
fn set_layout_attributes(geometry: Option<&BucketGeometry>, buffer: &Buffer, attribs: &mut VertexAttributeArray) {
    let layout_size = size_of::<crate::buckets::symbol_bucket::SymbolLayoutVertex>() as u32;
    let layout_data = bucket_vertices(geometry, &buffer.attribute_data);
    if let Some(attr) = attribs.set(ID_SYMBOL_POS_OFFSET_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 0, 0, layout_size, AttributeDataType::Short4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_DATA_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 8, 0, layout_size, AttributeDataType::UShort4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 16, 0, layout_size, AttributeDataType::Short4);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout_data)), 24, 0, layout_size, AttributeDataType::UShort2);
    }
    if let Some(attr) = attribs.set(ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(layout_data), 28, 0, layout_size, AttributeDataType::Float);
    }
}

/// Sets the dynamic layout vertices, which placement updates: the layer's
/// own when it has them.
fn set_projected_pos_attribute(buffer: &Buffer, layer_id: &str, attribs: &mut VertexAttributeArray) {
    set_placed_attribute(
        attribs,
        ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
        buffer.dynamic_attribute_data_for(layer_id),
        AttributeDataType::Float3,
    );
}

/// Sets the opacity vertices, which placement updates.
fn set_fade_opacity_attribute(buffer: &Buffer, attribs: &mut VertexAttributeArray) {
    set_placed_attribute(
        attribs,
        ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
        &buffer.opacity_attribute_data,
        AttributeDataType::Float,
    );
}

/// Sets attribute `id` of `attribs` to read `vertices`, which placement
/// updates, as `type_`: into the vertices it reads where they are as many,
/// so that the upload writes them over their buffer, or else into vertices
/// of their own. The bucket keeps `vertices`, so the attribute's copy only
/// stages them for the upload.
fn set_placed_attribute<V: bytemuck::Pod>(
    attribs: &mut VertexAttributeArray,
    id: usize,
    vertices: &[V],
    type_: AttributeDataType,
) {
    let updated = attribs
        .get(id)
        .and_then(|attr| attr.get_shared_raw_data())
        .and_then(|shared| {
            let any: &dyn Any = &**shared;
            any.downcast_ref::<VertexVector<V>>()
        })
        .is_some_and(|shared| shared.update(vertices));
    if updated {
        return;
    }
    if let Some(attr) = attribs.set(id, -1, AttributeDataType::Invalid, 0) {
        let shared = Rc::new(VertexVector::from_vec_staged(vertices.to_vec()));
        attr.set_shared_raw_data(Some(shared), 0, 0, size_of::<V>() as u32, type_);
    }
}

/// A draw the layer keeps of a tile: the halo or the fill of the text or the
/// icons of one of the bucket's buffers, or of one of its segments.
struct SymbolDraw {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    data: SymbolDrawData,
    /// Whether placement shows any of the draw's symbols. A draw that shows
    /// none doesn't draw, and doesn't copy the bucket's attributes again
    /// until it shows some.
    visible: bool,
    /// Gives the draw the textures of its tile's atlases.
    atlases: Rc<AtlasBinding>,
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
/// tile's bucket, and those of its collision boxes and circles.
#[derive(Default)]
pub(crate) struct SymbolTile {
    draws: Vec<SymbolDraw>,
    collision: Vec<CollisionDraw>,
}

/// The layer's drawing: what it keeps of each tile, its draws of the frame,
/// each of the draw at its index among its tile's, and as its overlay those
/// of the tiles' collision boxes and circles, from the first tile that has
/// any, each of the collision draw at its index among its tile's.
#[derive(Default)]
pub(crate) struct SymbolDrawing {
    tiles: TileMap<SymbolTile>,
    frame: Option<LayerDraws<usize>>,
    overlay: Option<LayerDraws<usize>>,
}

impl SymbolDrawing {
    /// The GPU objects of `draw`, the draw at `index` of its tile, with what
    /// gives them the textures of the tile's atlases.
    fn objects<'a>(tiles: &'a mut TileMap<SymbolTile>, draw: &FrameDraw, index: usize) -> Option<DrawObjectsMut<'a>> {
        let draw = tiles.get_mut(&draw.tile?)?.draws.get_mut(index)?;
        Some(DrawObjectsMut { objects: &mut draw.objects, atlases: Some(&*draw.atlases) })
    }

    /// The GPU objects of `draw`, the collision draw at `index` of its tile.
    fn collision_objects<'a>(
        tiles: &'a mut TileMap<SymbolTile>,
        draw: &FrameDraw,
        index: usize,
    ) -> Option<DrawObjectsMut<'a>> {
        Some((&mut tiles.get_mut(&draw.tile?)?.collision.get_mut(index)?.draw.objects).into())
    }

    /// Draws the pass's draws of the layer's draw group, if it has it.
    fn render_frame(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(&mut self.tiles, Self::objects, gpu, parameters);
            order.step(parameters);
        }
    }

    /// Draws the pass's draws of the overlay, if the layer has it.
    fn render_overlay(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(overlay) = &self.overlay {
            overlay.render(&mut self.tiles, Self::collision_objects, gpu, parameters);
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
        for draws in [&self.frame, &self.overlay].into_iter().flatten() {
            visit(DrawGroup::Draws(draws.frame()));
        }
    }

    fn clear(&mut self) {
        self.tiles.clear();
        for draws in [&mut self.frame, &mut self.overlay].into_iter().flatten() {
            draws.clear();
        }
    }

    fn upload(&mut self, context: &Context) {
        if let Some(frame) = &self.frame {
            frame.upload(&mut self.tiles, Self::objects, context);
        }
        if let Some(overlay) = &self.overlay {
            overlay.upload(&mut self.tiles, Self::collision_objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        match order {
            DrawOrder::BottomUp => {
                self.render_frame(gpu, parameters, order);
                self.render_overlay(gpu, parameters, order);
            }
            DrawOrder::TopDown => {
                self.render_overlay(gpu, parameters, order);
                self.render_frame(gpu, parameters, order);
            }
        }
    }
}

/// A draw of a tile's collision boxes or circles, with the translation of
/// the symbols they belong to.
struct CollisionDraw {
    draw: KeptDraw,
    translate: [f32; 2],
    translate_anchor: TranslateAnchorType,
}

/// A draw's entries in the layer's storage buffers.
struct SymbolUniforms {
    draw_ubo: SymbolDrawableUBO,
    tile_props: SymbolTilePropsUBO,
    projection: ProjectionUBO,
}

/// The size of the texture `objects` binds at `texture_id`, zero for none.
fn texture_size(objects: &DrawObjects, texture_id: usize) -> [f32; 2] {
    objects.texture(texture_id).map_or([0.0, 0.0], |texture| {
        let size = texture.borrow().size();
        [size.width as f32, size.height as f32]
    })
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into the draws of a tile, `draws`, all of which read the same
/// attributes (a halo and a fill): their triangles, and the attributes,
/// which the first of them that is visible copies and the others then read.
fn refresh_draws(
    draws: &mut [SymbolDraw],
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
) {
    let mut source = None;
    for (index, draw) in draws.iter_mut().enumerate() {
        if refresh_draw(draw, geometry, bucket, layer_id, evaluated, refresh, source.is_none()) {
            source = Some(index);
        }
    }
    let Some(source) = source else { return };
    let (before, rest) = draws.split_at_mut(source);
    let Some((source, after)) = rest.split_first_mut() else { return };
    for draw in before.iter_mut().chain(after) {
        draw.objects.share_vertex_attributes(&source.objects);
    }
}

/// Copies again what placement, sorting or feature state changed of the
/// bucket into a draw of it: its triangles, and where `copy_attributes`, its
/// attributes. Returns whether it copied the attributes.
fn refresh_draw(
    draw: &mut SymbolDraw,
    geometry: Option<&BucketGeometry>,
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
            set_projected_pos_attribute(buffer, layer_id, attrs);
        }
        if refresh.opacity {
            set_fade_opacity_attribute(buffer, attrs);
        }
        if refresh.paint
            && let Some(paint_props) = bucket.paint_properties.get(layer_id)
        {
            let mut properties_as_uniforms = StringIDSetsPair::default();
            set_binder_attributes(geometry, is_text, paint_props, evaluated, attrs, &mut properties_as_uniforms);
        }
    }

    if refresh.sort && !buffer.triangles.is_empty() {
        let segments = match draw.data.segment {
            Some(i) => buffer.segments.get(i..=i).unwrap_or_default(),
            None => &buffer.segments,
        };
        let segments = segments.iter().map(SegmentBase::copy).collect();
        draw.objects.replace_indexes(released_indexes(&buffer.triangles), segments);
    }
    copy_attributes
}

/// One segment, or all the segments, of a buffer of a tile's bucket, which
/// the update builds draws of in the order of [`SegmentGroup::less`].
struct SegmentGroup {
    sort_key: f32,
    type_: SymbolType,
    /// The tile's position among the layer's tiles ([`LayerTiles::get`](crate::render_layer::LayerTiles::get)).
    tile_index: usize,
    tile_id: UnwrappedTileID,
    overscaled_z: u8,
    /// One segment (`sortFeaturesByKey`) or all segments of the buffer.
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
    /// The overlay's draws as the update makes them.
    collision_list: DrawList<(), usize>,
    /// The segments of the tiles the update builds draws of, as it sorts
    /// them, kept from frame to frame for their memory.
    renderable_segments: Vec<SegmentGroup>,
    /// The buckets the layer places, in placement order (`placementData`).
    placement_data: Vec<BucketPlacementData>,
    /// The buckets another layer places and this layer draws (`followerData`).
    follower_data: Vec<BucketPlacementData>,

    icon_size: f32,
    text_size: f32,

    symbol_icon_group: Option<ShaderGroupPtr>,
    symbol_sdf_group: Option<ShaderGroupPtr>,
    symbol_text_and_icon_group: Option<ShaderGroupPtr>,

    collision_box_group: Option<ShaderGroupPtr>,
    collision_circle_group: Option<ShaderGroupPtr>,
    /// The passes the layer's draws were built for.
    draws_passes: RenderPass,
    /// bench: the indexes of the buckets `HeldBuckets` holds for the layer.
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
        let source_id = &self.base.style.base().source;
        let data = |feature_index, sort_key_range| BucketPlacementData {
            bucket: key,
            tile: *tile,
            feature_index,
            source_id: source_id.clone(),
            sort_key_range,
            text_translate,
            icon_translate,
        };
        if bucket.bucket_leader_id != self.base.style.base().id {
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
            // std::upper_bound by sort key
            let sort_position = self.placement_data.partition_point(|lhs| {
                let key = lhs.sort_key_range.expect("sort key range").sort_key;
                sort_key_range.sort_key.partial_cmp(&key) != Some(std::cmp::Ordering::Less)
            });
            self.placement_data.insert(sort_position, layer_data);
        }
    }

    /// bench: [`add_bucket`](Self::add_bucket) of the bucket that
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
            tile: *tile.get_overscaled_tile_id(),
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
            collision_list: DrawList::default(),
            renderable_segments: Vec::new(),
            placement_data: Vec::new(),
            follower_data: Vec::new(),
            icon_size: 1.0,
            text_size: 16.0,
            symbol_icon_group: None,
            symbol_sdf_group: None,
            symbol_text_and_icon_group: None,
            collision_box_group: None,
            collision_circle_group: None,
            draws_passes: RenderPass::None,
            #[cfg(feature = "bench")]
            held: Vec::new(),
        }
    }

    /// Builds the tile's collision draws anew, and makes the overlay for the
    /// first tile that has collision boxes or circles.
    fn rebuild_collision(
        &mut self,
        tile_id: &OverscaledTileID,
        bucket: &SymbolBucket,
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    ) {
        let mut collision = Vec::new();
        let mut has_collision = false;
        for is_text in [false, true] {
            has_collision |= self.add_collision_draws(&mut collision, bucket, evaluated, is_text);
        }
        if has_collision {
            self.drawing.overlay.get_or_insert_default();
            if let Some(entry) = self.drawing.tiles.entry(tile_id) {
                entry.value.get_or_insert_with(SymbolTile::default).collision = collision;
            }
        } else if let Some(cached) = self.drawing.tiles.get_mut(tile_id) {
            cached.collision.clear();
        }
    }

    /// Adds the draws of the collision boxes and circles of the bucket's
    /// text or icons to `collision`. Returns whether it has any.
    fn add_collision_draws(
        &self,
        collision: &mut Vec<CollisionDraw>,
        bucket: &SymbolBucket,
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
        is_text: bool,
    ) -> bool {
        let (has_collision_box, has_collision_circle) = if is_text {
            (bucket.has_text_collision_box_data(), bucket.has_text_collision_circle_data())
        } else {
            (bucket.has_icon_collision_box_data(), bucket.has_icon_collision_circle_data())
        };
        if !has_collision_box && !has_collision_circle {
            return false;
        }

        let values = if is_text {
            text_property_values(evaluated, &bucket.layout)
        } else {
            icon_property_values(evaluated, &bucket.layout)
        };
        let state = &bucket.state;
        let mut add = |buffer: &CollisionBuffer, group: &Option<ShaderGroupPtr>, mode: DrawMode| {
            let Some(group) = group else {
                return;
            };
            if buffer.vertices.is_empty() || buffer.indices.is_empty() {
                return;
            }
            let geometry = DrawGeometry {
                vertices: get_collision_vertex_attributes(buffer),
                vertex_count: buffer.vertices.len(),
                mode,
                indexes: released_indexes(&buffer.indices),
                segments: buffer.segments.iter().map(SegmentBase::copy).collect(),
                instances: None,
            };
            let objects = DrawObjects::new(
                group.shader(&Default::default(), self.base.projection_variant),
                ColorMode::alpha_blended(),
                CullFaceMode::disabled(),
                geometry,
                Textures::default(),
            );
            collision.push(CollisionDraw {
                draw: KeptDraw::new(objects),
                translate: values.translate,
                translate_anchor: values.translate_anchor,
            });
        };

        if has_collision_box {
            let collision_box = if is_text { &state.text_collision_box } else { &state.icon_collision_box };
            if let Some(collision_box) = collision_box {
                add(collision_box, &self.collision_box_group, DrawMode::Lines(1.0));
            }
        }
        if has_collision_circle {
            let collision_circle = if is_text { &state.text_collision_circle } else { &state.icon_collision_circle };
            if let Some(collision_circle) = collision_circle {
                add(collision_circle, &self.collision_circle_group, DrawMode::Triangles());
            }
        }
        true
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
        let state = parameters.state;
        let evaluated = &layer.paint;
        let zoom = state.get_zoom() as f32;
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
            Projection::Mercator.get_projection_data(state, &tile_id, &get_screen_matrix(*translate.uniform()))
        } else {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false;
            get_projection_data(
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
        projection.translate = [0.0, 0.0];
        let matrix = projection.fallback_matrix;

        // from symbol_program, makeValues
        let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
        let pitch_with_map = data.pitch_alignment == AlignmentType::Map;
        let rotate_with_map = data.rotation_alignment == AlignmentType::Map;
        let along_line = data.along_line;
        let has_variable_placement =
            data.bucket_variable_placement && (is_text || data.text_fit != IconTextFitType::None);
        let label_plane_matrix = if along_line || has_variable_placement {
            let mut m: Mat4 = [0.0; 16];
            matrix::identity(&mut m);
            m
        } else {
            get_label_plane_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units)
        };
        let gl_coord_matrix = get_gl_coord_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);
        let gamma_scale =
            if pitch_with_map { (state.get_pitch().cos() as f32) * state.get_camera_to_center_distance() } else { 1.0 };

        // Line label rotation happens in `updateLineLabels` /
        // `reprojectLineLabels`. Pitched point labels are automatically
        // rotated by the label plane matrix projection. Unpitched point
        // labels need to have their rotation applied after projection.
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
                texsize: texture_size(&draw.objects, ID_SYMBOL_IMAGE_TEXTURE),
                texsize_icon: texture_size(&draw.objects, ID_SYMBOL_IMAGE_ICON_TEXTURE),
                is_text_prop: i32::from(is_text),
                rotate_symbol: i32::from(rotate_in_shader),
                pitch_with_map: i32::from(pitch_with_map),
                is_size_zoom_constant: i32::from(size.is_zoom_constant),
                is_size_feature_constant: i32::from(size.is_feature_constant),
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
                pitched_scale: state.get_projection().circle_radius_correction(state) as f32,
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
    /// symbols, in `passes`, and of their collision boxes and circles, and
    /// writes the buffers they read.
    fn make_frame_draws<'a>(
        &mut self,
        tiles: impl Iterator<Item = FrameTile<'a, Symbol>>,
        passes: RenderPass,
        paint_parameters: &PaintParameters<'_>,
        context: &Context,
    ) {
        let layer_id = self.base.style.base().id.as_str();
        let layer = &*self.values.evaluated;
        for tile in tiles {
            let tile_id = tile.id();
            let Some(cached) = self.drawing.tiles.get_mut(&tile_id) else {
                continue;
            };
            let bucket = tile.bucket(&*self.base.style).map(|bucket| bucket.bucket);
            debug_assert!(bucket.is_some() || cached.draws.is_empty());
            for (index, draw) in cached.draws.iter().enumerate().filter(|(_, draw)| draw.visible) {
                let Some(uniforms) =
                    bucket.and_then(|bucket| Self::uniforms(layer_id, layer, &tile_id, bucket, draw, paint_parameters))
                else {
                    continue;
                };
                let frame_draw = FrameDraw { pass: passes, ..FrameDraw::new(Some(tile_id), draw.id) };
                self.draw_list.push_part(frame_draw, index, uniforms);
            }
            for (index, draw) in cached.collision.iter_mut().enumerate() {
                write_collision_uniforms(draw, &tile_id, paint_parameters);
                let frame_draw =
                    FrameDraw { pass: passes, depth: DrawDepth::Off, ..FrameDraw::new(Some(tile_id), draw.draw.id) };
                self.collision_list.push_part(frame_draw, index, ());
            }
        }

        if let Some(frame) = &mut self.drawing.frame {
            self.draw_list.finish(frame, context, |writer| write_uniforms(&layer.paint, writer));
        } else {
            self.draw_list.clear();
        }
        // Each collision draw has its own GPU objects and buffers, so no two
        // share an order, and the overlay's buffers are none.
        if let Some(overlay) = &mut self.drawing.overlay {
            self.collision_list.finish(overlay, context, |_| {});
        } else {
            self.collision_list.clear();
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
    writer.uniform(ID_SYMBOL_EVALUATED_PROPS_UBO, &props);
    writer.storage(ID_SYMBOL_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
    writer.storage(ID_SYMBOL_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
    writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
}

/// Writes the buffers of a collision draw of the tile `tile_id`: its matrix,
/// and the scale of its boxes' and circles' extrusions.
fn write_collision_uniforms(draw: &mut CollisionDraw, tile_id: &OverscaledTileID, parameters: &PaintParameters<'_>) {
    const NEAR_CLIPPED: bool = false;
    const IN_VIEWPORT_PIXEL_UNITS: bool = false;
    let context = parameters.context;
    let unwrapped = tile_id.to_unwrapped();
    let projection = get_projection_data(
        &unwrapped,
        parameters,
        draw.translate,
        draw.translate_anchor,
        NEAR_CLIPPED,
        IN_VIEWPORT_PIXEL_UNITS,
        None,
        false,
    );

    let zoom = parameters.state.get_zoom();
    let pixel_ratio = unwrapped.pixels_to_tile_units(1.0, zoom as f32);
    let scale = 2f64.powf(zoom - f64::from(tile_id.overscaled_z)) as f32;
    let extrude_scale = [
        parameters.pixels_to_gl_units[0] / (pixel_ratio * scale),
        parameters.pixels_to_gl_units[1] / (pixel_ratio * scale),
    ];
    let draw_ubo = CollisionDrawableUBO { matrix: cast_matrix(&projection.fallback_matrix) };
    let tile_props_ubo =
        CollisionTilePropsUBO { extrude_scale, overscale_factor: tile_id.overscale_factor() as f32, pad1: 0.0 };
    let buffers = &mut draw.draw.objects.uniform_buffers;
    buffers.create_or_update_pod(ID_COLLISION_DRAWABLE_UBO, &draw_ubo, context);
    buffers.create_or_update_pod(ID_COLLISION_TILE_PROPS_UBO, &tile_props_ubo, context);
    buffers.create_or_update_pod(ID_COLLISION_PROJECTION_UBO, &to_projection_ubo(&projection), context);
}

fn get_collision_vertex_attributes(buffer: &CollisionBuffer) -> VertexAttributeArray {
    let mut attrs = VertexAttributeArray::new();
    let layout_stride = size_of::<CollisionBoxLayoutVertex>() as u32;
    let layout = shared_vertices(&buffer.vertices);
    let dynamic_stride = size_of::<CollisionBoxDynamicVertex>() as u32;
    let dynamic = shared_vertices(&buffer.dynamic_vertices);
    if let Some(attr) = attrs.set(ID_COLLISION_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout)), 0, 0, layout_stride, AttributeDataType::Short2);
    }
    if let Some(attr) = attrs.set(ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout)), 4, 0, layout_stride, AttributeDataType::Short2);
    }
    if let Some(attr) = attrs.set(ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&layout)), 8, 0, layout_stride, AttributeDataType::Short2);
    }
    if let Some(attr) = attrs.set(ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(layout), 12, 0, layout_stride, AttributeDataType::Float);
    }
    if let Some(attr) = attrs.set(ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(Rc::clone(&dynamic)), 0, 0, dynamic_stride, AttributeDataType::UShort2);
    }
    if let Some(attr) = attrs.set(ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
        attr.set_shared_raw_data(Some(dynamic), 4, 0, dynamic_stride, AttributeDataType::Float2);
    }
    attrs
}

impl TypedLayer for RenderSymbolLayer {
    type Type = Symbol;
    type Drawing = SymbolDrawing;

    /// Symbols draw the lower of two tiles' overlapping symbols over the
    /// higher's.
    const TILE_ORDER: TileOrder = TileOrder::ByY;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Makes the frame's draws of the layer: those of each of its tiles,
    /// which it builds for a tile it keeps none of and refreshes where the
    /// tile's bucket changed, and the collision boxes and circles it draws
    /// over them, and writes the buffers they read.
    fn update_draws(
        &mut self,
        paint_parameters: &PaintParameters<'_>,
        _atlases: &Atlases,
        _update_parameters: &UpdateParameters,
        source: Option<LayerSource<'_, Symbol>>,
    ) {
        /// The atlases' textures given to one tile's draws.
        #[derive(Default)]
        struct TileAtlases {
            text: Option<Rc<AtlasBinding>>,
            icon: Option<Rc<AtlasBinding>>,
        }

        let (shaders, context, transform) =
            (&paint_parameters.static_data.shaders, paint_parameters.context, paint_parameters.state);
        self.update_projection_variant(transform);
        let passes = self.base.passes;
        let Some(render_tiles) = source.and_then(LayerSource::tiles).filter(|tiles| !tiles.is_empty()) else {
            self.remove_all_draws();
            return;
        };
        if passes == RenderPass::None {
            self.remove_all_draws();
            return;
        }
        if passes != self.draws_passes {
            self.remove_all_draws();
            self.draws_passes = passes;
        }
        self.drawing.frame.get_or_insert_default();

        if self.symbol_icon_group.is_none() {
            self.symbol_icon_group = shaders.shader_group(SYMBOL_ICON_SHADER_NAME);
        }
        if self.symbol_sdf_group.is_none() {
            self.symbol_sdf_group = shaders.shader_group(SYMBOL_SDF_SHADER_NAME);
        }
        if self.symbol_text_and_icon_group.is_none() {
            self.symbol_text_and_icon_group = shaders.shader_group(SYMBOL_TEXT_AND_ICON_SHADER_NAME);
        }
        if self.collision_box_group.is_none() {
            self.collision_box_group = shaders.shader_group("CollisionBoxShader");
        }
        if self.collision_circle_group.is_none() {
            self.collision_circle_group = shaders.shader_group("CollisionCircleShader");
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

        let current_zoom = transform.get_zoom() as f32;
        let style = Arc::clone(&self.base.style);
        let layer_id: &str = &style.base().id;

        let mut renderable_segments = std::mem::take(&mut self.renderable_segments);
        renderable_segments.clear();

        for (tile_index, tile) in render_tiles.iter_indexed() {
            let tile_id = tile.id();

            let Some(mut entry) = self.drawing.tiles.entry(&tile_id) else {
                continue;
            };
            let render_data = tile.bucket_for_pass(&*style, passes);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;

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
                let geometry = tile.bucket_geometry();
                for draws in cached.draws.chunk_by_mut(|a, b| a.attributes_key() == b.attributes_key()) {
                    refresh_draws(draws, geometry, bucket, layer_id, evaluated, refresh);
                }
                self.rebuild_collision(&tile_id, bucket, evaluated);
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

            let Some(atlases) = tile.atlas_textures() else {
                continue;
            };
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

            self.rebuild_collision(&tile_id, bucket, evaluated);
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
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();
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
            let Some(render_data) = tile.bucket_for_pass(&*style, passes) else {
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
            let Some(bucket_paint_properties) = bucket.paint_properties.get(self.id()) else {
                continue;
            };

            let bucket_layout = &*bucket.layout;
            let values = if is_text {
                text_property_values(evaluated, bucket_layout)
            } else {
                icon_property_values(evaluated, bucket_layout)
            };

            let Some(atlases) = tile.atlas_textures() else {
                debug_assert!(false);
                continue;
            };

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            let mut vertex_attribs = VertexAttributeArray::new();
            update_tile_attributes(
                tile.bucket_geometry(),
                buffer,
                self.id(),
                is_text,
                bucket_paint_properties,
                evaluated,
                &mut vertex_attribs,
                &mut properties_as_uniforms,
            );

            let text_halo = evaluated.text_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.text_halo_width.constant_or(&1.0) != 0.0;
            let text_fill = evaluated.text_color.constant_or(&Color::black()).a > 0.0;

            let icon_halo = evaluated.icon_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.icon_halo_width.constant_or(&1.0) != 0.0;
            let icon_fill = evaluated.icon_color.constant_or(&Color::black()).a > 0.0;

            // The shaders, halo first, with whether each is a halo's and
            // draws.
            let shader_groups = if is_text {
                let shader_group =
                    if bucket.icons_in_text { &self.symbol_text_and_icon_group } else { &self.symbol_sdf_group };
                vec![(shader_group, true, text_halo), (shader_group, false, text_fill)]
            } else if sdf_icons {
                vec![(&self.symbol_sdf_group, true, icon_halo), (&self.symbol_sdf_group, false, icon_fill)]
            } else {
                vec![(&self.symbol_icon_group, false, true)]
            };

            let segments = match group.segment {
                Some(i) => std::slice::from_ref(&buffer.segments[i]),
                None => &buffer.segments,
            };
            for (shader_group, is_halo, _) in shader_groups.into_iter().filter(|(_, _, drawn)| *drawn) {
                let Some(shader_group) = shader_group else {
                    continue;
                };

                // The draws of a tile share the atlases' textures.
                let tile_atlases = tile_atlases.entry(tile.tile.id).or_default();
                let draw_atlases = if is_text {
                    tile_atlases.text.get_or_insert_with(|| {
                        let text_size_is_zoom_constant =
                            bucket.text_size_binder.evaluate_for_zoom(current_zoom).is_zoom_constant;
                        Rc::new(AtlasBinding::new(
                            Some(Rc::clone(atlases)),
                            Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                            Some(ID_SYMBOL_IMAGE_TEXTURE),
                            is_text,
                            false,
                            values.rotation_alignment,
                            false,
                            text_size_is_zoom_constant,
                        ))
                    })
                } else {
                    tile_atlases.icon.get_or_insert_with(|| {
                        let icon_scaled = bucket_layout.icon_size.constant_or(&1.0) != 1.0 || bucket.icons_need_linear;
                        Rc::new(AtlasBinding::new(
                            Some(Rc::clone(atlases)),
                            Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                            Some(ID_SYMBOL_IMAGE_TEXTURE),
                            is_text,
                            sdf_icons,
                            values.rotation_alignment,
                            icon_scaled,
                            false,
                        ))
                    })
                };

                // The halo and the fill read the same attributes.
                let geometry = DrawGeometry {
                    vertices: copy_shared_attributes(&vertex_attribs),
                    vertex_count: buffer.attribute_data.len(),
                    mode: DrawMode::Triangles(),
                    indexes: released_indexes(&buffer.triangles),
                    segments: segments.iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                let shader = shader_group.shader(&properties_as_uniforms, symbol_projection_variant);
                let textures = draw_atlases.textures(&shader);
                let objects = DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), geometry, textures);

                let draw = SymbolDraw {
                    id: SimpleIdentity::new(),
                    objects,
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
                    atlases: Rc::clone(draw_atlases),
                };
                if let Some(entry) = self.drawing.tiles.entry(&tile_id) {
                    entry.value.get_or_insert_with(SymbolTile::default).draws.push(draw);
                }
            }
        }

        self.renderable_segments = renderable_segments;
        self.make_frame_draws(render_tiles.iter(), passes, paint_parameters, context);
    }

    /// Collects the symbol buckets of the tiles the layer places, and of
    /// those whose buckets another layer places and this one draws.
    fn update(&mut self, params: &mut LayerUpdateParameters<'_>, tiles: Option<LayerTiles<'_, Symbol>>) {
        let Some(source) = params.source else {
            return;
        };
        self.placement_data.clear();
        self.follower_data.clear();
        let (Some(tiles), Some(source)) = (tiles, source.tiles()) else {
            return;
        };
        let translates = self.symbol_translates();
        for tile in tiles.iter() {
            let Some(bucket) = tile.bucket(&*self.base.style) else {
                continue;
            };
            let key = BucketKey { source, tile: tile.id(), slot: tile.tile.slot(), bucket: bucket.bucket_index };
            let feature_index = || tile.feature_index().cloned();
            self.add_bucket(&PlacedTile::new(tile.tile), key, bucket.bucket, feature_index, translates);
        }
    }

    fn release(&mut self) {
        self.placement_data.clear();
    }
}

impl RenderLayer for RenderSymbolLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let mut properties = self.values.evaluate(parameters);

        if SymbolLayerPaintPropertyOverrides::has_overrides(&self.values.style.layout.text_field) {
            mln_style::format_section_override::set_overrides(
                &self.values.style.layout,
                &mut properties.paint,
                &self.values.unevaluated,
                &parameters.global_state,
            );
        }

        let evaluated = &properties.paint;
        let has_icon_opacity = evaluated.icon_color.constant_or(&SymbolPaintProperties::Defaults::icon_color()).a > 0.0
            || evaluated.icon_halo_color.constant_or(&SymbolPaintProperties::Defaults::icon_halo_color()).a > 0.0;
        let has_text_opacity = evaluated.text_color.constant_or(&SymbolPaintProperties::Defaults::text_color()).a > 0.0
            || evaluated.text_halo_color.constant_or(&SymbolPaintProperties::Defaults::text_halo_color()).a > 0.0;

        self.base.passes =
            if (evaluated.icon_opacity.constant_or(&1.0) > 0.0 && has_icon_opacity && self.icon_size > 0.0)
                || (evaluated.text_opacity.constant_or(&1.0) > 0.0 && has_text_opacity && self.text_size > 0.0)
            {
                RenderPass::Translucent
            } else {
                RenderPass::None
            };
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn placement_data(&self) -> &[BucketPlacementData] {
        &self.placement_data
    }

    fn follower_data(&self) -> &[BucketPlacementData] {
        &self.follower_data
    }
}
