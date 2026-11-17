// C++ drawables share the bucket's vertex vectors and see placement's
// changes through them (`updateModified`). Here a drawable gets copies of
// the vectors; when placement, sorting or feature state change the bucket
// (the changes the tile's entry in `LayerDraws` collects), `update`
// replaces just those attributes (opacity, dynamic positions, binders) and
// the triangle indices of the tile's existing drawables, as C++
// `updateTileDrawable` does.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE,
    ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE,
    ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE,
    ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_gfx::vertex_vector::VertexVector;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, ShaderRegistry};
use mln_map_types::TransformState;
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
use mln_util::mat4::{Mat4, matrix};

use super::collision_layer_tweaker::{CollisionDrawableData, CollisionLayerTweaker};
use super::symbol_layer_tweaker::SymbolLayerTweaker;
use super::{LayerTiles, shared_vertices, update_tile};
use crate::bucket_geometry::{BucketGeometry, bucket_vertices, paint_vertices, released_indexes};
use crate::buckets::symbol_bucket::{
    Buffer, CollisionBoxDynamicVertex, CollisionBoxLayoutVertex, CollisionBuffer, PaintProperties, SymbolBucket,
};
use crate::draw_cache::{BucketChanges, LayerDraws};
use crate::drawable_builder::DrawableBuilder;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::context::Context;
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweaker;
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::symbol_layout::SymbolLayout;
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    BucketPlacementData, LayerPrepareParameters, LayerValues, RenderLayer, RenderLayerBase, SymbolTranslate, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_tile::RenderTile;
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct SymbolDrawableData {
    pub(crate) is_halo: bool,
    pub(crate) bucket_variable_placement: bool,
    pub(crate) symbol_type: SymbolType,
    pub(crate) pitch_alignment: AlignmentType,
    pub(crate) rotation_alignment: AlignmentType,
    pub(crate) along_line: bool,
    pub(crate) text_fit: IconTextFitType,
    pub(crate) is_offset: bool,
    /// The bucket segment the drawable draws (`sortFeaturesByKey`), or all
    /// of them.
    pub(crate) segment: Option<usize>,
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
    bucket: &SymbolBucket,
    buffer: &Buffer,
    layer_id: &str,
    is_text: bool,
    paint_props: &PaintProperties,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    attribs: &mut VertexAttributeArray,
    properties_as_uniforms: &mut StringIDSetsPair,
) {
    set_binder_attributes(geometry, bucket, is_text, paint_props, evaluated, attribs, properties_as_uniforms);
    set_static_and_dynamic_attributes(geometry, bucket, buffer, layer_id, attribs);
}

/// The data-driven paint property half of `updateTileAttributes`.
fn set_binder_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
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
        let shared = paint_vertices(geometry, bucket, ib, b.depends_on_feature_state());
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
        let shared = paint_vertices(geometry, bucket, ib, b.depends_on_feature_state());
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
    bucket: &SymbolBucket,
    buffer: &Buffer,
    layer_id: &str,
    attribs: &mut VertexAttributeArray,
) {
    set_layout_attributes(geometry, bucket, buffer, attribs);
    set_projected_pos_attribute(buffer, layer_id, attribs);
    set_fade_opacity_attribute(buffer, attribs);
}

/// The static half of `updateTileAttributes`: the layout vertices.
fn set_layout_attributes(
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    buffer: &Buffer,
    attribs: &mut VertexAttributeArray,
) {
    let layout_size = size_of::<crate::buckets::symbol_bucket::SymbolLayoutVertex>() as u32;
    let layout_data = bucket_vertices(geometry, bucket, &buffer.attribute_data);
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

/// Replaces the attributes (and indices) of an existing drawable that
/// placement, sorting or feature state changed. Halo and fill drawables share
/// their attribute array, which is refreshed once (`refreshed_attributes`).
fn update_tile_drawable(
    drawable: &mut crate::drawable::Drawable,
    geometry: Option<&BucketGeometry>,
    bucket: &SymbolBucket,
    layer_id: &str,
    evaluated: &SymbolPaintProperties::PossiblyEvaluated,
    refresh: BucketChanges,
    refreshed_attributes: &mut Vec<*const ()>,
) {
    let Some(crate::drawable::DrawableData::Symbol(draw_data)) = &drawable.data else {
        return;
    };
    let mut draw_data = *draw_data;
    let is_text = draw_data.symbol_type == SymbolType::Text;
    let sdf_icons = draw_data.symbol_type == SymbolType::IconSDF;

    let state = bucket.state();

    // This property can be set after the initial appearance of the tile, as
    // part of the layout process.
    if draw_data.bucket_variable_placement != state.has_variable_placement {
        draw_data.bucket_variable_placement = state.has_variable_placement;
        drawable.data = Some(crate::drawable::DrawableData::Symbol(draw_data));
    }

    let buffer = if is_text {
        &state.text
    } else if sdf_icons {
        &state.sdf_icon
    } else {
        &state.icon
    };

    // A drawable whose symbols are all hidden by placement draws nothing.
    // Its attributes aren't refreshed while it is disabled, so it refreshes
    // all of them when placement shows it again.
    let was_enabled = drawable.enabled;
    if refresh.opacity || refresh.sort {
        drawable.enabled = buffer.segment_has_visible_vertices(draw_data.segment);
    }
    if !drawable.enabled {
        return;
    }
    let refresh = if was_enabled { refresh } else { BucketChanges::ALL };

    if !refresh.any() {
        return;
    }

    if let Some(attrs) = drawable.vertex_attributes.clone() {
        let key = Rc::as_ptr(&attrs).cast::<()>();
        if !refreshed_attributes.contains(&key) {
            refreshed_attributes.push(key);
            let mut attrs = attrs.borrow_mut();
            if refresh.dynamic {
                set_projected_pos_attribute(buffer, layer_id, &mut attrs);
            }
            if refresh.opacity {
                set_fade_opacity_attribute(buffer, &mut attrs);
            }
            if refresh.paint
                && let Some(paint_props) = bucket.paint_properties.get(layer_id)
            {
                let paint_props = paint_props.read();
                let mut properties_as_uniforms = StringIDSetsPair::default();
                set_binder_attributes(
                    geometry,
                    bucket,
                    is_text,
                    &paint_props,
                    evaluated,
                    &mut attrs,
                    &mut properties_as_uniforms,
                );
            }
        }
    }

    if refresh.sort && !buffer.triangles.is_empty() {
        let segments: Vec<crate::drawable::UniqueDrawSegment> = match draw_data.segment {
            Some(i) => buffer.segments.get(i).map(|s| vec![s]).unwrap_or_default(),
            None => buffer.segments.iter().collect(),
        }
        .into_iter()
        .map(|segment| Box::new(crate::drawable::DrawSegment::new(DrawMode::Triangles(), segment.copy())))
        .collect();
        drawable.set_index_data(Some(released_indexes(&buffer.triangles)), segments);
    }
}

/// A `SegmentGroup` (a `RenderableSegment` plus its segments).
struct SegmentGroup {
    sort_key: f32,
    type_: SymbolType,
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
    /// The group of the collision boxes and circles, drawn for debugging,
    /// which draws after the layer's group.
    collision_tile_layer_group: Option<LayerGroup>,
    collision_layer_tweaker: Option<Box<dyn LayerTweaker>>,
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

    /// Adds `bucket`, of the tile at `tile_index` of `owner`, to what the
    /// layer places: when the layer leads the bucket, to its placement data,
    /// with the tile's feature index, in order of sort key; otherwise to its
    /// follower data.
    pub(crate) fn add_placement_data(
        &mut self,
        owner: &Rc<Vec<RenderTile>>,
        tile_index: usize,
        bucket: Arc<SymbolBucket>,
        feature_index: impl FnOnce() -> Option<Arc<FeatureIndex>>,
        (text_translate, icon_translate): (SymbolTranslate, SymbolTranslate),
    ) {
        if bucket.bucket_leader_id != self.get_id() {
            self.follower_data.push(BucketPlacementData {
                bucket,
                tile_owner: Rc::clone(owner),
                tile_index,
                feature_index: None,
                source_id: self.base.style.base().source.clone(),
                sort_key_range: None,
                text_translate,
                icon_translate,
            });
            return;
        }
        // Only place this layer if it's the "group leader" for the bucket
        let feature_index = feature_index();

        if bucket.sort_key_ranges.is_empty() {
            self.placement_data.push(BucketPlacementData {
                bucket,
                tile_owner: Rc::clone(owner),
                tile_index,
                feature_index,
                source_id: self.base.style.base().source.clone(),
                sort_key_range: None,
                text_translate,
                icon_translate,
            });
            return;
        }
        for sort_key_range in &bucket.sort_key_ranges {
            let layer_data = BucketPlacementData {
                bucket: Arc::clone(&bucket),
                tile_owner: Rc::clone(owner),
                tile_index,
                feature_index: feature_index.clone(),
                source_id: self.base.style.base().source.clone(),
                sort_key_range: Some(*sort_key_range),
                text_translate,
                icon_translate,
            };
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
            placement_data: Vec::new(),
            follower_data: Vec::new(),
            icon_size: 1.0,
            text_size: 16.0,
            symbol_icon_group: None,
            symbol_sdf_group: None,
            symbol_text_and_icon_group: None,
            collision_box_group: None,
            collision_circle_group: None,
            collision_tile_layer_group: None,
            collision_layer_tweaker: None,
        }
    }

    /// Removes the tile's collision drawables.
    fn remove_collision_tile(&mut self, render_pass: RenderPass, tile_id: &OverscaledTileID) {
        if let Some(group) = &mut self.collision_tile_layer_group {
            group.remove_drawables(render_pass, tile_id);
        }
    }

    /// Adds the collision box and circle drawables of the tile's text or
    /// icons.
    fn add_collision_drawables(
        &mut self,
        passes: RenderPass,
        tile_id: OverscaledTileID,
        bucket: &SymbolBucket,
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
        is_text: bool,
    ) {
        let (has_collision_box, has_collision_circle) = if is_text {
            (bucket.has_text_collision_box_data(), bucket.has_text_collision_circle_data())
        } else {
            (bucket.has_icon_collision_box_data(), bucket.has_icon_collision_circle_data())
        };
        if !has_collision_box && !has_collision_circle {
            return;
        }

        if self.collision_layer_tweaker.is_none() {
            self.collision_layer_tweaker = Some(Box::new(CollisionLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
        }

        let values = if is_text {
            text_property_values(evaluated, &bucket.layout)
        } else {
            icon_property_values(evaluated, &bucket.layout)
        };
        let suffix = if is_text { "text/" } else { "icon/" };
        let layer_collision_prefix = format!("{}-collision/", self.get_id());

        let mut collision_builder = DrawableBuilder::boxed();
        collision_builder.set_sub_layer_index(0);
        collision_builder.set_enable_depth(false);
        collision_builder.set_render_pass(passes);
        collision_builder.set_cull_face_mode(CullFaceMode::disabled());
        collision_builder.set_color_mode(&ColorMode::alpha_blended());

        let state = bucket.state();
        let mut add = |buffer: &CollisionBuffer, group: &Option<ShaderGroupPtr>, mode: DrawMode, name: &str| {
            let Some(group) = group else {
                return;
            };
            let shader = group.shader(&Default::default(), self.base.projection_variant);
            collision_builder.set_drawable_name(format!("{layer_collision_prefix}{suffix}{name}"));
            collision_builder.set_shader(Some(shader));
            collision_builder.set_raw_vertices(Vec::new(), buffer.vertices.len(), AttributeDataType::Short2);
            collision_builder.set_vertex_attributes(Some(get_collision_vertex_attributes(buffer)));
            collision_builder.set_segments_shared(mode, Some(released_indexes(&buffer.indices)), &buffer.segments);
            collision_builder.flush();
        };

        if has_collision_box {
            let collision_box = if is_text { &state.text_collision_box } else { &state.icon_collision_box };
            if let Some(collision_box) = collision_box {
                add(collision_box, &self.collision_box_group, DrawMode::Lines(1.0), "box");
            }
        }
        if has_collision_circle {
            let collision_circle = if is_text { &state.text_collision_circle } else { &state.icon_collision_circle };
            if let Some(collision_circle) = collision_circle {
                add(collision_circle, &self.collision_circle_group, DrawMode::Triangles(), "circle");
            }
        }
        drop(state);

        // add drawables to layer group
        let tweaker = self.collision_layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);
        let tile_group = self.collision_tile_layer_group.get_or_insert_with(LayerGroup::tiles);
        for mut drawable in collision_builder.clear_drawables() {
            drawable.tile_id = Some(tile_id);
            drawable.tweaker = tweaker;
            drawable.data = Some(crate::drawable::DrawableData::Collision(CollisionDrawableData {
                translate: values.translate,
                translate_anchor: values.translate_anchor,
            }));
            tile_group.add_tile_drawable(passes, &tile_id, drawable);
        }
    }
}

fn get_collision_vertex_attributes(buffer: &CollisionBuffer) -> mln_gfx::vertex_attribute::VertexAttributeArrayPtr {
    let vertex_attrs = VertexAttributeArray::new_ptr();
    {
        let mut attrs = vertex_attrs.borrow_mut();
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
    }
    vertex_attrs
}

impl TypedLayer for RenderSymbolLayer {
    type Type = Symbol;

    fn parts(&self) -> (&RenderLayerBase, &LayerValues<Symbol>) {
        (&self.base, &self.values)
    }

    fn parts_mut(&mut self) -> (&mut RenderLayerBase, &mut LayerValues<Symbol>) {
        (&mut self.base, &mut self.values)
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

    fn tweak(&mut self, parameters: &PaintParameters<'_>, draws: &mut LayerDraws) {
        if !self.base.is_renderable {
            return;
        }
        let base = &mut self.base;
        let tiles = LayerTiles::new(&*base.style, base.render_tiles.as_ref());
        if let (Some(tweaker), Some(group)) = (&mut base.layer_tweaker, &mut draws.group) {
            tweaker.execute(group, &tiles, parameters);
        }
        if let (Some(tweaker), Some(group)) = (&mut self.collision_layer_tweaker, &mut self.collision_tile_layer_group)
        {
            tweaker.execute(group, &tiles, parameters);
        }
    }

    fn layer_groups<'a>(&'a mut self, draws: &'a mut LayerDraws, groups: &mut Vec<&'a mut LayerGroup>) {
        if self.base.is_renderable {
            groups.extend([&mut draws.group, &mut self.collision_tile_layer_group].into_iter().flatten());
        }
    }

    fn get_placement_data(&self) -> &[BucketPlacementData] {
        &self.placement_data
    }

    fn get_follower_data(&self) -> &[BucketPlacementData] {
        &self.follower_data
    }

    fn release_render_data(&mut self, draws: &mut LayerDraws) {
        crate::render_layer::release_render_data_base(self, draws);
        self.placement_data.clear();
    }

    fn remove_all_drawables(&mut self, draws: &mut LayerDraws) {
        draws.clear_drawables();
        if let Some(group) = &mut self.collision_tile_layer_group {
            group.clear_drawables();
        }
    }

    fn handle_wrap_jump(&mut self, wrap_delta: i16, draws: &mut LayerDraws) {
        draws.shift_wraps(wrap_delta);
        if let Some(group) = &mut self.collision_tile_layer_group {
            group.shift_wraps(wrap_delta);
        }
    }

    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) {
        let Some(source) = params.source else {
            self.base.render_tiles = None;
            params.draws.clear_tiles();
            return;
        };
        self.base.render_tiles = source.get_render_tiles_sorted_by_y_position();
        self.base.render_tiles_owner = source.get_raw_render_tiles();

        self.update_tile_entries(params.draws);
        if self.base.render_tiles.is_some() {
            self.add_render_passes_from_tiles();
        }

        self.placement_data.clear();
        self.follower_data.clear();

        let translates = self.symbol_translates();
        let Some(render_tiles) = self.base.render_tiles.clone() else {
            return;
        };
        let owner = Rc::clone(render_tiles.owner());
        for render_tile in render_tiles.iter() {
            let Some(bucket_ptr) = render_tile.get_bucket(&*self.base.style) else {
                continue;
            };
            let Some(bucket) = crate::bucket::recover::<SymbolBucket>(bucket_ptr) else {
                continue;
            };
            let Some(tile_index) = owner.iter().position(|t| std::ptr::eq(t, render_tile)) else {
                continue;
            };
            self.add_placement_data(&owner, tile_index, bucket, || render_tile.get_feature_index(), translates);
        }
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        _context: &Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        draws: &mut LayerDraws,
    ) {
        /// The tweakers built for one tile.
        #[derive(Default)]
        struct TileInfo {
            text_tweaker: Option<Rc<DrawableAtlasesTweaker>>,
            icon_tweaker: Option<Rc<DrawableAtlasesTweaker>>,
        }

        self.update_projection_variant(state, draws);
        let passes = self.base.passes;
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables(draws);
            return;
        };
        if passes == RenderPass::None {
            self.remove_all_drawables(draws);
            return;
        }

        // Set up a layer group, which this fills, and puts back when it's
        // done.
        let mut layer_group = draws.group.take().unwrap_or_else(LayerGroup::tiles);

        if self.base.layer_tweaker.is_none() {
            self.base.layer_tweaker = Some(Box::new(SymbolLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
        }

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

        // remove drawables that are dropped out of scope: if the render pass
        // has changed or the tile has dropped out of the cover set, remove
        // it.
        let dropped = |drawable: &mut crate::drawable::Drawable| {
            drawable.render_pass != passes || drawable.tile_id.is_some_and(|id| !draws.has_tile(&id))
        };
        layer_group.remove_drawables_if(dropped);
        if let Some(collision_group) = &mut self.collision_tile_layer_group {
            collision_group.remove_drawables_if(dropped);
        }

        let layout = &self.values.style.layout;
        let sort_features_by_key = layout.symbol_sort_key.is_expression();
        let constant_sort_key = layout.symbol_sort_key.is_constant();
        // Screen-space symbols are drawn over the view, not projected onto
        // the map.
        let screen_space_prop = &layout.symbol_screen_space;
        let is_screen_space = if screen_space_prop.is_constant() {
            *screen_space_prop.as_constant()
        } else {
            SymbolLayoutProperties::Defaults::symbol_screen_space()
        };
        let symbol_projection_variant =
            if is_screen_space { ProjectionVariant::Mercator } else { self.base.projection_variant };
        let is_offset = !layout.icon_offset.is_undefined();

        let current_zoom = state.get_zoom() as f32;
        let layer_prefix = format!("{}/", self.get_id());
        let tweaker = self.base.layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);

        let mut renderable_segments: Vec<SegmentGroup> = Vec::new();

        let tiles: Vec<&RenderTile> = render_tiles.iter().collect();
        for (tile_index, tile) in tiles.iter().enumerate() {
            let tile_id = *tile.get_overscaled_tile_id();

            let style = Arc::clone(&self.base.style);
            let render_data = super::render_data_for_pass(tile, &*style, passes);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                layer_group.remove_drawables(passes, &tile_id);
                self.remove_collision_tile(passes, &tile_id);
                continue;
            };
            let Some(bucket) = render_data.bucket::<Symbol>() else {
                continue;
            };

            let prev_bucket_id = draws.bucket(&tile_id);
            if prev_bucket_id.is_some_and(|prev| prev != bucket.base.bucket_id) {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                layer_group.remove_drawables(passes, &tile_id);
                self.remove_collision_tile(passes, &tile_id);
            }
            draws.set_bucket(&tile_id, bucket.base.bucket_id);
            // What changed since the drawables were built or last refreshed.
            let refresh = draws.take_changes(&tile_id);

            debug_assert!(bucket.paint_properties.contains_key(self.get_id()));
            if !bucket.paint_properties.contains_key(self.get_id()) {
                continue;
            }

            let Some(evaluated) = render_data.layer::<Symbol>().map(|layer| &layer.paint) else {
                continue;
            };

            // If we already have drawables for this tile, update them.
            let layer_id = self.get_id().to_owned();
            let mut refreshed_attributes: Vec<*const ()> = Vec::new();
            let update_existing = |drawable: &mut crate::drawable::Drawable| {
                if drawable.tweaker != tweaker {
                    // This drawable was produced on a previous style/bucket,
                    // and should not be updated.
                    return false;
                }
                update_tile_drawable(
                    drawable,
                    tile.get_bucket_geometry(),
                    bucket,
                    &layer_id,
                    evaluated,
                    refresh,
                    &mut refreshed_attributes,
                );
                true
            };
            if update_tile(&mut layer_group, passes, &tile_id, update_existing) {
                // re-create collision drawables
                self.remove_collision_tile(passes, &tile_id);
                self.add_collision_drawables(passes, tile_id, bucket, evaluated, false);
                self.add_collision_drawables(passes, tile_id, bucket, evaluated, true);
                continue;
            }

            let mut serial_key = 1.0_f32;
            let state_guard = bucket.state();
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
                            tile_id: tile.id,
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
                        tile_id: tile.id,
                        overscaled_z: tile_id.overscaled_z,
                        segment: None,
                    });
                    serial_key += 1.0;
                }
            };

            let Some(atlases) = tile.get_atlas_textures() else {
                continue;
            };
            let has_icon_texture = atlases.icon().is_some();
            let has_glyph_texture = atlases.glyph().is_some();

            if !state_guard.icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state_guard.icon, SymbolType::IconRGBA);
            }
            if !state_guard.sdf_icon.segments.is_empty() && has_icon_texture {
                add_renderables(&state_guard.sdf_icon, SymbolType::IconSDF);
            }
            if !state_guard.text.segments.is_empty() && has_glyph_texture {
                add_renderables(&state_guard.text, SymbolType::Text);
            }

            drop(state_guard);
            self.add_collision_drawables(passes, tile_id, bucket, evaluated, false);
            self.add_collision_drawables(passes, tile_id, bucket, evaluated, true);
        }

        // std::multiset<SegmentGroup>: a stable sort keeps equal elements in
        // insertion order, as multiset::emplace does.
        renderable_segments.sort_by(|a, b| {
            if SegmentGroup::less(a, b) {
                std::cmp::Ordering::Less
            } else if SegmentGroup::less(b, a) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        // We'll be processing renderables across tiles, potentially
        // out-of-order, so keep track of some things by tile ID so we don't
        // have to re-build them multiple times.
        let mut tile_cache: HashMap<UnwrappedTileID, TileInfo> = HashMap::default();
        let mut builder: Option<crate::drawable_builder::UniqueDrawableBuilder> = None;
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();

        for group in &renderable_segments {
            let is_text = group.type_ == SymbolType::Text;
            let sdf_icons = group.type_ == SymbolType::IconSDF;

            let tile = tiles[group.tile_index];
            let tile_id = tile.id.overscale_to(group.overscaled_z);
            let style = Arc::clone(&self.base.style);
            let Some(render_data) = super::render_data_for_pass(tile, &*style, passes) else {
                continue;
            };
            let Some(bucket) = render_data.bucket::<Symbol>() else {
                continue;
            };
            let state_guard = bucket.state();
            let buffer = if is_text {
                &state_guard.text
            } else if sdf_icons {
                &state_guard.sdf_icon
            } else {
                &state_guard.icon
            };

            if buffer.triangles.is_empty() {
                continue;
            }
            let visible = buffer.segment_has_visible_vertices(group.segment);

            let Some(evaluated) = render_data.layer::<Symbol>().map(|layer| &layer.paint) else {
                continue;
            };
            let Some(bucket_paint_properties) = bucket.paint_properties.get(self.get_id()) else {
                continue;
            };
            let bucket_paint_properties = bucket_paint_properties.read();

            let bucket_layout = &*bucket.layout;
            let values = if is_text {
                text_property_values(evaluated, bucket_layout)
            } else {
                icon_property_values(evaluated, bucket_layout)
            };

            let Some(atlases) = tile.get_atlas_textures() else {
                debug_assert!(false);
                continue;
            };

            let tile_info = tile_cache.entry(tile.id).or_default();

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            let vertex_attribs = VertexAttributeArray::new_ptr();
            update_tile_attributes(
                tile.get_bucket_geometry(),
                bucket,
                buffer,
                self.get_id(),
                is_text,
                &bucket_paint_properties,
                evaluated,
                &mut vertex_attribs.borrow_mut(),
                &mut properties_as_uniforms,
            );

            let text_halo = evaluated.text_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.text_halo_width.constant_or(&1.0) != 0.0;
            let text_fill = evaluated.text_color.constant_or(&Color::black()).a > 0.0;

            let icon_halo = evaluated.icon_halo_color.constant_or(&Color::black()).a > 0.0
                && evaluated.icon_halo_width.constant_or(&1.0) != 0.0;
            let icon_fill = evaluated.icon_color.constant_or(&Color::black()).a > 0.0;

            if let Some(builder) = builder.as_mut() {
                builder.set_atlases(None);
            }

            let mut draw = |shader_group: &Option<ShaderGroupPtr>, is_halo: bool, suffix: &str| {
                let Some(shader_group) = shader_group else {
                    return;
                };

                // We can use the same tweakers for all the segments in a tile
                if is_text && tile_info.text_tweaker.is_none() {
                    let text_size_is_zoom_constant =
                        bucket.text_size_binder.evaluate_for_zoom(current_zoom).is_zoom_constant;
                    tile_info.text_tweaker = Some(Rc::new(DrawableAtlasesTweaker::new(
                        Some(Rc::clone(atlases)),
                        Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                        Some(ID_SYMBOL_IMAGE_TEXTURE),
                        is_text,
                        false,
                        values.rotation_alignment,
                        false,
                        text_size_is_zoom_constant,
                    )));
                }
                if !is_text && tile_info.icon_tweaker.is_none() {
                    let icon_scaled = bucket_layout.icon_size.constant_or(&1.0) != 1.0 || bucket.icons_need_linear;
                    tile_info.icon_tweaker = Some(Rc::new(DrawableAtlasesTweaker::new(
                        Some(Rc::clone(atlases)),
                        Some(ID_SYMBOL_IMAGE_ICON_TEXTURE),
                        Some(ID_SYMBOL_IMAGE_TEXTURE),
                        is_text,
                        sdf_icons,
                        values.rotation_alignment,
                        icon_scaled,
                        false,
                    )));
                }

                let builder = builder.get_or_insert_with(|| {
                    let mut builder = DrawableBuilder::boxed();
                    builder.set_sub_layer_index(0);
                    builder.set_render_pass(passes);
                    builder.set_cull_face_mode(CullFaceMode::disabled());
                    builder.set_depth_type(DepthMaskType::ReadOnly);
                    builder.set_color_mode(&if (passes.underlying_type() & RenderPass::Translucent.underlying_type())
                        != 0
                    {
                        ColorMode::alpha_blended()
                    } else {
                        ColorMode::unblended()
                    });
                    builder
                });

                let shader = shader_group.shader(&properties_as_uniforms, symbol_projection_variant);
                builder.set_shader(Some(shader));

                builder.set_atlases(if is_text {
                    tile_info.text_tweaker.clone()
                } else {
                    tile_info.icon_tweaker.clone()
                });
                builder.set_drawable_name(format!("{layer_prefix}{suffix}"));
                builder.set_vertex_attributes(Some(Rc::clone(&vertex_attribs)));

                builder.set_raw_vertices(Vec::new(), buffer.attribute_data.len(), AttributeDataType::Short4);

                let triangle_indices = Some(released_indexes(&buffer.triangles));
                match group.segment {
                    Some(i) => builder.set_segments_shared(
                        DrawMode::Triangles(),
                        triangle_indices,
                        std::slice::from_ref(&buffer.segments[i]),
                    ),
                    None => builder.set_segments_shared(DrawMode::Triangles(), triangle_indices, &buffer.segments),
                }

                builder.flush();

                for mut drawable in builder.clear_drawables() {
                    drawable.tile_id = Some(tile_id);
                    drawable.tweaker = tweaker;

                    let has_variable_placement = state_guard.has_variable_placement;
                    drawable.data = Some(crate::drawable::DrawableData::Symbol(SymbolDrawableData {
                        is_halo,
                        bucket_variable_placement: has_variable_placement,
                        symbol_type: group.type_,
                        pitch_alignment: values.pitch_alignment,
                        rotation_alignment: values.rotation_alignment,
                        along_line: values.along_line,
                        text_fit: *bucket_layout.icon_text_fit.uniform(),
                        is_offset,
                        segment: group.segment,
                    }));
                    drawable.enabled = visible;

                    layer_group.add_tile_drawable(passes, &tile_id, drawable);
                }
            };

            if is_text {
                if bucket.icons_in_text {
                    if text_halo {
                        draw(&self.symbol_text_and_icon_group, true, "halo");
                    }
                    if text_fill {
                        draw(&self.symbol_text_and_icon_group, false, "fill");
                    }
                } else {
                    if text_halo {
                        draw(&self.symbol_sdf_group, true, "halo");
                    }
                    if text_fill {
                        draw(&self.symbol_sdf_group, false, "fill");
                    }
                }
            } else if sdf_icons {
                // icons
                if icon_halo {
                    draw(&self.symbol_sdf_group, true, "halo");
                }
                if icon_fill {
                    draw(&self.symbol_sdf_group, false, "fill");
                }
            } else {
                draw(&self.symbol_icon_group, false, "icon");
            }
        }
        draws.group = Some(layer_group);
    }
}
