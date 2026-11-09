// Ports src/mln/layout/symbol_instance.hpp and src/mln/layout/symbol_instance.cpp.
//
// Not ported: the MLN_SYMBOL_GUARDS memory-corruption guards (check values,
// checkIndex, forceFail): they detect C++ memory corruption, which safe
// Rust cannot have.

use std::sync::Arc;

use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style_types::image::ImageMap;
use mln_style_types::{SymbolAnchorType, SymbolPlacementType, VariableAnchorOffsetCollection};
use mln_text::glyph::{Shaping, WritingModeType};
use mln_text::shaping::PositionedIcon;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::Padding;
use serde::{Deserialize, Serialize};

use crate::geometry::anchor::Anchor;
use crate::geometry::feature_index::IndexedSubfeature;
use crate::text::collision_feature::CollisionFeature;
use crate::text::quads::{SymbolQuads, get_glyph_quads, get_icon_quads};

/// `enum class SymbolContent : uint8_t` (bit flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolContent(pub u8);

#[allow(non_upper_case_globals)]
impl SymbolContent {
    pub const None: SymbolContent = SymbolContent(0);
    pub const Text: SymbolContent = SymbolContent(1 << 0);
    pub const IconRGBA: SymbolContent = SymbolContent(1 << 1);
    pub const IconSDF: SymbolContent = SymbolContent(1 << 2);
}

impl std::ops::BitOr for SymbolContent {
    type Output = SymbolContent;
    fn bitor(self, rhs: SymbolContent) -> SymbolContent {
        SymbolContent(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for SymbolContent {
    type Output = SymbolContent;
    fn bitand(self, rhs: SymbolContent) -> SymbolContent {
        SymbolContent(self.0 & rhs.0)
    }
}

impl std::ops::BitOrAssign for SymbolContent {
    fn bitor_assign(&mut self, rhs: SymbolContent) {
        self.0 |= rhs.0;
    }
}

/// `ShapedTextOrientations`. C++ `right` is a reference to `horizontal`;
/// here `right()`/`right_mut()` return `horizontal`.
#[derive(Clone, Debug, Default)]
pub struct ShapedTextOrientations {
    pub horizontal: Shaping,
    pub vertical: Shaping,
    // The following are used with variable text placement on.
    pub center: Shaping,
    pub left: Shaping,
    pub single_line: bool,
}

impl ShapedTextOrientations {
    pub fn right(&self) -> &Shaping {
        &self.horizontal
    }

    pub fn right_mut(&mut self) -> &mut Shaping {
        &mut self.horizontal
    }
}

fn get_any_shaping(shaped_text_orientations: &ShapedTextOrientations) -> &Shaping {
    if shaped_text_orientations.right().is_some() {
        return shaped_text_orientations.right();
    }
    if shaped_text_orientations.center.is_some() {
        return &shaped_text_orientations.center;
    }
    if shaped_text_orientations.left.is_some() {
        return &shaped_text_orientations.left;
    }
    if shaped_text_orientations.vertical.is_some() {
        return &shaped_text_orientations.vertical;
    }
    &shaped_text_orientations.horizontal
}

/// `SymbolInstanceSharedData`.
#[derive(Debug, Default)]
pub struct SymbolInstanceSharedData {
    pub line: GeometryCoordinates,
    // Note: When singleLine == true, only `rightJustifiedGlyphQuads` is populated.
    pub right_justified_glyph_quads: SymbolQuads,
    pub center_justified_glyph_quads: SymbolQuads,
    pub left_justified_glyph_quads: SymbolQuads,
    pub vertical_glyph_quads: SymbolQuads,
    pub icon_quads: Option<SymbolQuads>,
    pub vertical_icon_quads: Option<SymbolQuads>,
}

impl SymbolInstanceSharedData {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        line: GeometryCoordinates,
        shaped_text_orientations: &ShapedTextOrientations,
        shaped_icon: &Option<PositionedIcon>,
        vertically_shaped_icon: &Option<PositionedIcon>,
        layout: &SymbolLayoutProperties::Evaluated,
        text_placement: SymbolPlacementType,
        text_offset: &[f32; 2],
        image_map: &ImageMap,
        icon_rotation: f32,
        icon_type: SymbolContent,
        has_icon_text_fit: bool,
        allow_vertical_placement: bool,
    ) -> Self {
        let mut data = SymbolInstanceSharedData { line, ..Default::default() };
        // Create the quads used for rendering the icon and glyphs.
        if let Some(shaped_icon) = shaped_icon {
            data.icon_quads = Some(get_icon_quads(shaped_icon, icon_rotation, icon_type, has_icon_text_fit));
            if let Some(vertically_shaped_icon) = vertically_shaped_icon {
                data.vertical_icon_quads =
                    Some(get_icon_quads(vertically_shaped_icon, icon_rotation, icon_type, has_icon_text_fit));
            }
        }

        let quads = |shaping: &Shaping| {
            get_glyph_quads(shaping, *text_offset, layout, text_placement, image_map, allow_vertical_placement)
        };

        let mut single_line_initialized = false;
        let shapings: [(&Shaping, u8); 3] = [
            (shaped_text_orientations.right(), 0),
            (&shaped_text_orientations.center, 1),
            (&shaped_text_orientations.left, 2),
        ];
        for (shaping, which) in shapings {
            if !shaping.is_some() {
                continue;
            }
            if !shaped_text_orientations.single_line {
                let q = quads(shaping);
                match which {
                    0 => data.right_justified_glyph_quads = q,
                    1 => data.center_justified_glyph_quads = q,
                    _ => data.left_justified_glyph_quads = q,
                }
                continue;
            }
            if !single_line_initialized {
                data.right_justified_glyph_quads = quads(shaping);
                single_line_initialized = true;
            }
        }

        if shaped_text_orientations.vertical.is_some() {
            data.vertical_glyph_quads = quads(&shaped_text_orientations.vertical);
        }
        data
    }

    pub fn empty(&self) -> bool {
        self.right_justified_glyph_quads.is_empty()
            && self.center_justified_glyph_quads.is_empty()
            && self.left_justified_glyph_quads.is_empty()
            && self.vertical_glyph_quads.is_empty()
    }
}

/// `SymbolInstance`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInstance {
    /// Port-only: not written by serde. The layout releases it before the
    /// bucket leaves the worker (`releaseSharedData`).
    #[serde(skip)]
    shared_data: Option<Arc<SymbolInstanceSharedData>>,
    anchor: Anchor,
    symbol_content: SymbolContent,
    right_justified_glyph_quads_size: usize,
    center_justified_glyph_quads_size: usize,
    left_justified_glyph_quads_size: usize,
    vertical_glyph_quads_size: usize,
    icon_quads_size: usize,
    text_collision_feature: CollisionFeature,
    icon_collision_feature: CollisionFeature,
    vertical_text_collision_feature: Option<CollisionFeature>,
    vertical_icon_collision_feature: Option<CollisionFeature>,
    writing_modes: WritingModeType,
    /// Index into the set of features included at layout time
    layout_feature_index: usize,
    /// Index into the underlying tile data feature set
    data_feature_index: usize,
    text_offset: [f32; 2],
    icon_offset: [f32; 2],
    key: Vec<u16>,
    pub placed_right_text_index: Option<usize>,
    pub placed_center_text_index: Option<usize>,
    pub placed_left_text_index: Option<usize>,
    pub placed_vertical_text_index: Option<usize>,
    pub placed_icon_index: Option<usize>,
    pub placed_vertical_icon_index: Option<usize>,
    text_box_scale: f32,
    text_variable_anchor_offset: Option<VariableAnchorOffsetCollection>,
    single_line: bool,
    cross_tile_id: u32,
}

impl SymbolInstance {
    pub const INVALID_CROSS_TILE_ID: u32 = u32::MAX;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        anchor: Anchor,
        shared_data: Arc<SymbolInstanceSharedData>,
        shaped_text_orientations: &ShapedTextOrientations,
        shaped_icon: &Option<PositionedIcon>,
        vertically_shaped_icon: &Option<PositionedIcon>,
        text_box_scale: f32,
        text_padding: f32,
        text_placement: SymbolPlacementType,
        text_offset: [f32; 2],
        icon_box_scale: f32,
        icon_padding: Padding,
        icon_offset: [f32; 2],
        indexed_feature: &IndexedSubfeature,
        layout_feature_index: usize,
        data_feature_index: usize,
        key: Vec<u16>,
        overscaling: f32,
        icon_rotation: f32,
        text_rotation: f32,
        text_variable_anchor_offset: &Option<VariableAnchorOffsetCollection>,
        allow_vertical_placement: bool,
        icon_type: SymbolContent,
    ) -> Self {
        // Create the collision features that will be used to check whether
        // this symbol instance can be placed As a collision approximation, we
        // can use either the vertical or any of the horizontal versions of
        // the feature
        let text_collision_feature = CollisionFeature::new_text(
            &shared_data.line,
            &anchor,
            get_any_shaping(shaped_text_orientations),
            text_box_scale,
            text_padding,
            text_placement,
            indexed_feature,
            overscaling,
            text_rotation,
        );
        let icon_collision_feature = CollisionFeature::new_icon(
            &shared_data.line,
            &anchor,
            shaped_icon.as_ref(),
            icon_box_scale,
            &icon_padding,
            indexed_feature,
            icon_rotation,
        );
        let mut symbol_content = icon_type;
        // 'hasText' depends on finding at least one glyph in the shaping
        // that's also in the GlyphPositionMap
        if !shared_data.empty() {
            symbol_content |= SymbolContent::Text;
        }
        let mut vertical_text_collision_feature = None;
        let mut vertical_icon_collision_feature = None;
        if allow_vertical_placement && shaped_text_orientations.vertical.is_some() {
            let vertical_point_label_angle = 90.0_f32;
            vertical_text_collision_feature = Some(CollisionFeature::new_text(
                &shared_data.line,
                &anchor,
                &shaped_text_orientations.vertical,
                text_box_scale,
                text_padding,
                text_placement,
                indexed_feature,
                overscaling,
                text_rotation + vertical_point_label_angle,
            ));
            if vertically_shaped_icon.is_some() {
                vertical_icon_collision_feature = Some(CollisionFeature::new_icon(
                    &shared_data.line,
                    &anchor,
                    vertically_shaped_icon.as_ref(),
                    icon_box_scale,
                    &icon_padding,
                    indexed_feature,
                    icon_rotation + vertical_point_label_angle,
                ));
            }
        }

        let right_justified_glyph_quads_size = shared_data.right_justified_glyph_quads.len();
        let center_justified_glyph_quads_size = shared_data.center_justified_glyph_quads.len();
        let left_justified_glyph_quads_size = shared_data.left_justified_glyph_quads.len();
        let vertical_glyph_quads_size = shared_data.vertical_glyph_quads.len();
        let icon_quads_size = shared_data.icon_quads.as_ref().map_or(0, |q| q.len());

        let mut writing_modes = WritingModeType::None;
        if right_justified_glyph_quads_size != 0
            || center_justified_glyph_quads_size != 0
            || left_justified_glyph_quads_size != 0
        {
            writing_modes = writing_modes | WritingModeType::Horizontal;
        }
        if vertical_glyph_quads_size != 0 {
            writing_modes = writing_modes | WritingModeType::Vertical;
        }

        SymbolInstance {
            shared_data: Some(shared_data),
            anchor,
            symbol_content,
            right_justified_glyph_quads_size,
            center_justified_glyph_quads_size,
            left_justified_glyph_quads_size,
            vertical_glyph_quads_size,
            icon_quads_size,
            text_collision_feature,
            icon_collision_feature,
            vertical_text_collision_feature,
            vertical_icon_collision_feature,
            writing_modes,
            layout_feature_index,
            data_feature_index,
            text_offset,
            icon_offset,
            key,
            placed_right_text_index: None,
            placed_center_text_index: None,
            placed_left_text_index: None,
            placed_vertical_text_index: None,
            placed_icon_index: None,
            placed_vertical_icon_index: None,
            text_box_scale,
            text_variable_anchor_offset: text_variable_anchor_offset.clone(),
            single_line: shaped_text_orientations.single_line,
            cross_tile_id: 0,
        }
    }

    fn shared(&self) -> &SymbolInstanceSharedData {
        self.shared_data.as_deref().expect("SymbolInstance: shared data released")
    }

    pub fn get_default_horizontal_placed_text_index(&self) -> Option<usize> {
        self.placed_right_text_index.or(self.placed_center_text_index).or(self.placed_left_text_index)
    }

    pub fn line(&self) -> &GeometryCoordinates {
        &self.shared().line
    }
    pub fn right_justified_glyph_quads(&self) -> &SymbolQuads {
        &self.shared().right_justified_glyph_quads
    }
    pub fn left_justified_glyph_quads(&self) -> &SymbolQuads {
        &self.shared().left_justified_glyph_quads
    }
    pub fn center_justified_glyph_quads(&self) -> &SymbolQuads {
        &self.shared().center_justified_glyph_quads
    }
    pub fn vertical_glyph_quads(&self) -> &SymbolQuads {
        &self.shared().vertical_glyph_quads
    }
    pub fn icon_quads(&self) -> &Option<SymbolQuads> {
        &self.shared().icon_quads
    }
    pub fn vertical_icon_quads(&self) -> &Option<SymbolQuads> {
        &self.shared().vertical_icon_quads
    }

    pub fn has_text(&self) -> bool {
        (self.symbol_content & SymbolContent::Text).0 != 0
    }
    pub fn has_icon(&self) -> bool {
        (self.symbol_content & SymbolContent::IconRGBA).0 != 0 || self.has_sdf_icon()
    }
    pub fn has_sdf_icon(&self) -> bool {
        (self.symbol_content & SymbolContent::IconSDF).0 != 0
    }

    pub fn release_shared_data(&mut self) {
        self.shared_data = None;
    }

    pub fn get_text_anchors(&self) -> Vec<SymbolAnchorType> {
        match &self.text_variable_anchor_offset {
            Some(offsets) => offsets.iter().map(|anchor_offset| anchor_offset.anchor_type).collect(),
            None => Vec::new(),
        }
    }

    pub fn get_anchor(&self) -> &Anchor {
        &self.anchor
    }
    pub fn get_right_justified_glyph_quads_size(&self) -> usize {
        self.right_justified_glyph_quads_size
    }
    pub fn get_center_justified_glyph_quads_size(&self) -> usize {
        self.center_justified_glyph_quads_size
    }
    pub fn get_left_justified_glyph_quads_size(&self) -> usize {
        self.left_justified_glyph_quads_size
    }
    pub fn get_vertical_glyph_quads_size(&self) -> usize {
        self.vertical_glyph_quads_size
    }
    pub fn get_icon_quads_size(&self) -> usize {
        self.icon_quads_size
    }
    pub fn get_text_collision_feature(&self) -> &CollisionFeature {
        &self.text_collision_feature
    }
    pub fn get_icon_collision_feature(&self) -> &CollisionFeature {
        &self.icon_collision_feature
    }
    pub fn get_vertical_text_collision_feature(&self) -> &Option<CollisionFeature> {
        &self.vertical_text_collision_feature
    }
    pub fn get_vertical_icon_collision_feature(&self) -> &Option<CollisionFeature> {
        &self.vertical_icon_collision_feature
    }
    pub fn get_writing_modes(&self) -> WritingModeType {
        self.writing_modes
    }
    pub fn get_layout_feature_index(&self) -> usize {
        self.layout_feature_index
    }
    pub fn get_data_feature_index(&self) -> usize {
        self.data_feature_index
    }
    pub fn get_text_offset(&self) -> [f32; 2] {
        self.text_offset
    }
    pub fn get_icon_offset(&self) -> [f32; 2] {
        self.icon_offset
    }
    pub fn get_key(&self) -> &[u16] {
        &self.key
    }
    pub fn get_text_box_scale(&self) -> f32 {
        self.text_box_scale
    }
    pub fn get_text_variable_anchor_offset(&self) -> &Option<VariableAnchorOffsetCollection> {
        &self.text_variable_anchor_offset
    }
    pub fn get_single_line(&self) -> bool {
        self.single_line
    }
    pub fn get_cross_tile_id(&self) -> u32 {
        self.cross_tile_id
    }
    pub fn set_cross_tile_id(&mut self, x: u32) {
        self.cross_tile_id = x;
    }
}
