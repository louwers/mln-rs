//! A feature's symbol at one anchor: what layout makes of it, and what its
//! bucket keeps for placement.

use std::num::NonZeroU32;
use std::sync::Arc;

use mln_style::feature::GeometryCoordinates;
use mln_style::types::{SymbolAnchorType, TextJustifyType, TextWritingModeType, VariableAnchorOffsetCollection};
use mln_text::anchor_justification;
use mln_text::glyph::Shaping;
use mln_wire::payload::pod;

use crate::geometry::anchor::Anchor;
use crate::placement::collision_feature::{CollisionBox, CollisionFeature};
use crate::text::quads::SymbolQuads;
use crate::util::pool::PoolRange;

/// The writing modes that a symbol's text, or a run of its quads, shows in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, mln_wire::Wire)]
pub(crate) enum WritingMode {
    #[default]
    None,
    Horizontal,
    Vertical,
    Both,
}

impl WritingMode {
    pub(crate) fn new(horizontal: bool, vertical: bool) -> Self {
        match (horizontal, vertical) {
            (false, false) => WritingMode::None,
            (true, false) => WritingMode::Horizontal,
            (false, true) => WritingMode::Vertical,
            (true, true) => WritingMode::Both,
        }
    }

    pub(crate) fn has_vertical(self) -> bool {
        matches!(self, WritingMode::Vertical | WritingMode::Both)
    }
}

/// The collision boxes and keys of a bucket's symbol instances, which they
/// hold ranges of.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct SymbolInstancePools {
    pub(crate) collision_boxes: Vec<CollisionBox>,
    /// The symbols' texts in UTF-8.
    #[wire(with = "pod")]
    pub(crate) keys: Vec<u8>,
}

impl SymbolInstancePools {
    pub(crate) fn boxes(&self, feature: &CollisionFeature) -> &[CollisionBox] {
        feature.boxes.of(&self.collision_boxes)
    }

    /// The symbol's text in UTF-8.
    pub(crate) fn key(&self, symbol: &SymbolInstance) -> &[u8] {
        symbol.key.of(&self.keys)
    }
}

/// The justification of a horizontal text run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Justification {
    Right,
    Center,
    Left,
}

impl Justification {
    /// The justifications, in the order layout adds their runs.
    pub(crate) const ALL: [Justification; 3] = [Justification::Right, Justification::Center, Justification::Left];

    /// The justification `text-justify: auto` takes at `anchor`.
    pub(crate) fn of_anchor(anchor: SymbolAnchorType) -> Self {
        Self::of(anchor_justification(anchor)).unwrap_or(Justification::Center)
    }

    /// `justify`, unless it is `auto`.
    pub(crate) fn of(justify: TextJustifyType) -> Option<Self> {
        match justify {
            TextJustifyType::Right => Some(Justification::Right),
            TextJustifyType::Center => Some(Justification::Center),
            TextJustifyType::Left => Some(Justification::Left),
            TextJustifyType::Auto => None,
        }
    }
}

impl From<Justification> for TextJustifyType {
    fn from(justification: Justification) -> Self {
        match justification {
            Justification::Right => TextJustifyType::Right,
            Justification::Center => TextJustifyType::Center,
            Justification::Left => TextJustifyType::Left,
        }
    }
}

/// A feature's text shaped for each justification it may show at, and
/// vertically. Single-line text looks the same at every justification, so
/// it has one horizontal shaping.
#[derive(Clone, Debug, Default)]
pub(crate) struct ShapedTextOrientations {
    pub(crate) horizontal: [Shaping; 3],
    pub(crate) vertical: Shaping,
    pub(crate) single_line: bool,
}

impl ShapedTextOrientations {
    pub(crate) fn get(&self, justification: Justification) -> &Shaping {
        &self.horizontal[justification as usize]
    }

    pub(crate) fn get_mut(&mut self, justification: Justification) -> &mut Shaping {
        &mut self.horizontal[justification as usize]
    }

    /// The first horizontal shaping there is. The justifications share one
    /// collision box, so any stands for all.
    pub(crate) fn any_horizontal(&self) -> Option<&Shaping> {
        self.horizontal.iter().find(|shaping| !shaping.is_empty())
    }

    /// The shaping the text's collision box is made from: a horizontal one,
    /// else the vertical one.
    pub(crate) fn any(&self) -> &Shaping {
        self.any_horizontal().unwrap_or(&self.vertical)
    }
}

/// The quads of a symbol's text and icon, which the symbols at each anchor
/// of a line share.
#[derive(Debug, Default)]
pub(crate) struct SymbolQuadSet {
    pub(crate) line: GeometryCoordinates,
    pub(crate) single_line: bool,
    /// By justification; single-line text has only right-justified quads.
    pub(crate) horizontal: [SymbolQuads; 3],
    pub(crate) vertical: SymbolQuads,
    pub(crate) icon: Option<SymbolQuads>,
    pub(crate) vertical_icon: Option<SymbolQuads>,
}

impl SymbolQuadSet {
    pub(crate) fn has_text(&self) -> bool {
        self.text_writing_modes() != WritingMode::None
    }

    /// The writing modes the text has quads in.
    pub(crate) fn text_writing_modes(&self) -> WritingMode {
        let horizontal = self.horizontal.iter().any(|quads| !quads.is_empty());
        let vertical = !self.vertical.is_empty();
        WritingMode::new(horizontal, vertical)
    }
}

/// A symbol as layout makes it: its bucket's symbol, whose runs the bucket
/// sets as it adds the quads, with the index of its feature among the
/// layout's.
#[derive(Debug)]
pub(crate) struct LaidOutSymbol {
    pub(crate) symbol: SymbolInstance,
    pub(crate) quads: Arc<SymbolQuadSet>,
    pub(crate) feature: usize,
    pub(crate) text_offset: [f32; 2],
    pub(crate) icon_offset: [f32; 2],
}

/// The runs of a symbol's text in its bucket's text buffer: each run is a
/// placed symbol, the quads of one justification or of the vertical text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, mln_wire::Wire)]
pub(crate) struct TextRuns {
    /// By justification. Single-line text has one run for all three.
    pub(crate) horizontal: [Option<u32>; 3],
    pub(crate) vertical: Option<u32>,
}

impl TextRuns {
    pub(crate) fn get(&self, justification: Justification) -> Option<usize> {
        self.horizontal[justification as usize].map(|run| run as usize)
    }

    /// The run a horizontal label shows when no justification was chosen.
    pub(crate) fn default_horizontal(&self) -> Option<usize> {
        self.horizontal.iter().flatten().next().map(|&run| run as usize)
    }

    /// Each distinct horizontal run, in buffer order.
    pub(crate) fn horizontal(&self) -> impl Iterator<Item = usize> + '_ {
        self.horizontal
            .iter()
            .enumerate()
            .filter_map(|(i, run)| run.filter(|run| !self.horizontal[..i].contains(&Some(*run))))
            .map(|run| run as usize)
    }

    /// Each distinct run, in buffer order, with its orientation.
    pub(crate) fn all(&self) -> impl Iterator<Item = (usize, TextWritingModeType)> + '_ {
        self.horizontal()
            .map(|run| (run, TextWritingModeType::Horizontal))
            .chain(self.vertical.map(|run| (run as usize, TextWritingModeType::Vertical)))
    }

    pub(crate) fn writing_modes(&self) -> WritingMode {
        let horizontal = self.horizontal.iter().any(Option::is_some);
        let vertical = self.vertical.is_some();
        WritingMode::new(horizontal, vertical)
    }
}

/// A symbol's text.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct TextPart {
    pub(crate) runs: TextRuns,
    pub(crate) collision: CollisionFeature,
    pub(crate) vertical_collision: Option<CollisionFeature>,
    pub(crate) box_scale: f32,
    /// The anchors placement tries, with their offsets; empty for a label
    /// at a fixed anchor.
    pub(crate) variable_anchors: Option<VariableAnchorOffsetCollection>,
}

impl TextPart {
    /// The variable anchors in the order to try them.
    pub(crate) fn anchors(&self) -> Vec<SymbolAnchorType> {
        self.variable_anchors.iter().flatten().map(|anchor_offset| anchor_offset.anchor_type).collect()
    }

    /// The collision feature of `vertical` text.
    pub(crate) fn collision(&self, vertical: bool) -> &CollisionFeature {
        if vertical { self.vertical_collision.as_ref().unwrap_or(&self.collision) } else { &self.collision }
    }
}

/// A symbol's icon.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct IconPart {
    /// The icon's run in its bucket's icon or SDF icon buffer.
    pub(crate) run: u32,
    /// The run of the icon fitted to vertical text.
    pub(crate) vertical_run: Option<u32>,
    pub(crate) collision: CollisionFeature,
    pub(crate) vertical_collision: Option<CollisionFeature>,
    pub(crate) sdf: bool,
}

impl IconPart {
    /// Each run, with its orientation.
    pub(crate) fn runs(&self) -> impl Iterator<Item = (usize, TextWritingModeType)> {
        std::iter::once((self.run as usize, TextWritingModeType::Horizontal))
            .chain(self.vertical_run.map(|run| (run as usize, TextWritingModeType::Vertical)))
    }

    /// The collision feature of the icon beside `vertical` text.
    pub(crate) fn collision(&self, vertical: bool) -> &CollisionFeature {
        if vertical { self.vertical_collision.as_ref().unwrap_or(&self.collision) } else { &self.collision }
    }
}

/// A feature's symbol at one anchor.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct SymbolInstance {
    pub(crate) anchor: Anchor,
    /// The symbol's text, which the cross-tile index matches symbols by, in
    /// its bucket's `SymbolInstancePools`.
    pub(crate) key: PoolRange,
    /// The index of the symbol's feature in its tile's layer data.
    pub(crate) data_feature_index: usize,
    /// `symbol-height-offset`, in meters above the ground.
    pub(crate) height_offset: f32,
    /// `None` until the cross-tile index assigns one, and for symbols it skips.
    pub(crate) cross_tile_id: Option<NonZeroU32>,
    /// The text, where it has glyphs to draw.
    pub(crate) text: Option<TextPart>,
    pub(crate) icon: Option<IconPart>,
}

impl SymbolInstance {
    pub(crate) fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    pub(crate) fn cross_tile_id(&self) -> Option<NonZeroU32> {
        self.cross_tile_id
    }

    pub(crate) fn set_cross_tile_id(&mut self, id: Option<NonZeroU32>) {
        self.cross_tile_id = id;
    }

    pub(crate) fn height_offset(&self) -> f32 {
        self.height_offset
    }

    pub(crate) fn has_text(&self) -> bool {
        self.text.is_some()
    }

    pub(crate) fn has_icon(&self) -> bool {
        self.icon.is_some()
    }
}
