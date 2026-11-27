//! Placing one symbol: its text, at its anchor or the first variable anchor
//! where it fits, in either writing mode; its icon; and both together.

use std::num::NonZeroU32;

use glam::Vec2;
use mln_engine::placement::Overlap;
use mln_engine::text::anchor_alignment;
use mln_style::types::{SymbolAnchorType, TextWritingModeType};

use super::collision::{self, LabelRun, PartCollision, PlacedFeatureResult};
use super::collision_feature::CollisionFeature;
use super::context::PlacementContext;
use super::{JointPlacement, Placer, VariableOffset};
use crate::layers::symbol::instance::{IconPart, SymbolInstance, TextPart};
use crate::layers::symbol::projection::evaluate_size_for_feature;

impl VariableOffset {
    /// The shift of the label from its anchor, along the label's own axes.
    pub(crate) fn layout_shift(&self) -> Vec2 {
        let shift = -(anchor_alignment(self.anchor) - 0.5) * Vec2::new(self.width, self.height);
        shift + Vec2::from(self.offset) * self.text_box_scale
    }

    /// The shift as a viewport offset, for boxes that aren't projected:
    /// turned with the map where the label turns with it.
    pub(crate) fn screen_shift(&self, rotate_with_map: bool, pitch_with_map: bool, bearing: f32) -> Vec2 {
        let shift = self.layout_shift();
        if rotate_with_map {
            Vec2::from_angle(if pitch_with_map { bearing } else { -bearing }).rotate(shift)
        } else {
            shift
        }
    }

    /// The shift the label draws at, at `text_size`.
    pub(crate) fn render_shift(&self, text_size: f32) -> Vec2 {
        let shift = -(anchor_alignment(self.anchor) - 0.5) * Vec2::new(self.width, self.height);
        (shift / self.text_box_scale + Vec2::from(self.offset)) * text_size
    }
}

/// Where a symbol's text went.
#[derive(Clone, Copy, Debug, Default)]
struct PlacedText {
    result: PlacedFeatureResult,
    /// Placed in the vertical writing mode.
    vertical: bool,
    /// The shift of the variable anchor it took.
    shift: Option<Vec2>,
}

/// A label to place at the first of its variable anchors where it fits.
#[derive(Clone, Copy)]
struct VariableLabel<'a> {
    id: NonZeroU32,
    text: &'a TextPart,
    /// The collision feature of the icon fitted to the text, which must fit
    /// at the anchor too.
    icon_feature: Option<&'a CollisionFeature>,
    anchors: &'a [SymbolAnchorType],
    run: &'a LabelRun<'a>,
}

/// The orientation a label placed `vertical` has.
fn orientation(vertical: bool) -> TextWritingModeType {
    if vertical { TextWritingModeType::Vertical } else { TextWritingModeType::Horizontal }
}

impl Placer<'_> {
    /// Places `symbol`, a symbol of `ctx`'s bucket, and records where.
    pub(super) fn place_symbol(&mut self, symbol: &SymbolInstance, ctx: &PlacementContext<'_>) -> JointPlacement {
        let Some(id) = symbol.cross_tile_id else {
            return JointPlacement::UNPLACED;
        };
        // A tile held to fade out shows none of its symbols, and doesn't
        // keep a tile that replaces it from placing them.
        if ctx.tile().hold_for_fade {
            return JointPlacement::UNPLACED;
        }
        self.run.text_boxes.clear();
        self.run.icon_boxes.clear();

        let text = symbol.text.as_ref().and_then(|text| self.place_text(id, symbol, text, ctx));
        let icon = symbol.icon.as_ref().map(|icon| self.place_icon(icon, text, ctx));
        let text_vertical = text.is_some_and(|text| text.vertical);
        let text_result = text.map(|text| text.result).unwrap_or_default();
        let (icon_result, icon_vertical) = icon.unwrap_or_default();
        let offscreen = text.is_none_or(|text| text.result.offscreen) && icon.is_none_or(|(icon, _)| icon.offscreen);

        // Each part waits for the other, unless it is optional or missing.
        let layout = ctx.layout();
        let text_needs_icon = symbol.has_icon() && !*layout.icon_optional.uniform();
        let icon_needs_text = symbol.has_text() && !*layout.text_optional.uniform();
        let mut place_text = text_result.placed;
        let mut place_icon = icon_result.placed;
        if text_needs_icon && icon_needs_text {
            place_text &= place_icon;
            place_icon = place_text;
        } else if text_needs_icon {
            place_text &= place_icon;
        } else if icon_needs_text {
            place_icon &= place_text;
        }

        let bucket_instance_id = ctx.bucket.bucket_instance_id;
        let group = ctx.collision_group;
        if place_text && let Some(text) = &symbol.text {
            collision::insert_feature(
                self.index,
                text.collision(text_vertical),
                &self.run.text_boxes,
                ctx.text_overlap_mode,
                *layout.text_ignore_placement.uniform(),
                bucket_instance_id,
                group,
            );
        }
        if place_icon && let Some(icon) = &symbol.icon {
            collision::insert_feature(
                self.index,
                icon.collision(icon_vertical),
                &self.run.icon_boxes,
                ctx.icon_overlap_mode,
                *layout.icon_ignore_placement.uniform(),
                bucket_instance_id,
                group,
            );
        }

        // A symbol behind the planet's horizon stays hidden even where it
        // may overlap. A placement from a tile that fades out gives way to
        // this one, from a tile that doesn't.
        let placement = JointPlacement {
            text: (place_text || ctx.always_show_text) && !text_result.occluded,
            icon: (place_icon || ctx.always_show_icon) && !icon_result.occluded,
            skip_fade: offscreen || ctx.just_reloaded,
        };
        self.run.next.placements.insert(id, placement);
        self.symbol_placed(symbol, ctx, placement);
        placement
    }

    /// Places `text`, the text of the symbol `id`; `None` where it has no
    /// horizontal run, or variable anchors it can't take.
    fn place_text(
        &mut self,
        id: NonZeroU32,
        symbol: &SymbolInstance,
        text: &TextPart,
        ctx: &PlacementContext<'_>,
    ) -> Option<PlacedText> {
        let state = &ctx.bucket.state;
        let run = &state.text.placed_symbols[text.runs.default_horizontal()?];
        let label = LabelRun {
            symbol: run,
            pools: &state.text.pools,
            font_size: evaluate_size_for_feature(&ctx.text_size, run),
        };
        let anchors = text.anchors();
        let placed = if anchors.is_empty() {
            self.place_in_writing_modes(text, ctx, |this, vertical| {
                if vertical && !(ctx.bucket.allow_vertical_placement && text.vertical_collision.is_some()) {
                    return PlacedText::default();
                }
                let feature = text.collision(vertical);
                let boxes = state.symbol_instance_pools.boxes(feature);
                this.run.text_boxes.clear();
                let result = collision::place_feature(
                    &*this.index,
                    &ctx.text_collision(),
                    feature,
                    boxes,
                    &label,
                    None,
                    &mut this.run.text_boxes,
                );
                if result.placed {
                    this.run.next.placed_orientations.entry(id).or_insert(orientation(vertical));
                }
                PlacedText { result, vertical, shift: None }
            })
        } else if !text.collision.along_line && !state.symbol_instance_pools.boxes(&text.collision).is_empty() {
            let anchors = self.anchors_in_order(id, anchors, ctx);
            let placed = self.place_in_writing_modes(text, ctx, |this, vertical| {
                if vertical && !(ctx.bucket.allow_vertical_placement && text.vertical_collision.is_some()) {
                    return PlacedText::default();
                }
                // The box `place_icon` places the icon fitted to the text in.
                let icon_feature = symbol
                    .icon
                    .as_ref()
                    .filter(|_| ctx.has_icon_text_fit && ctx.icon_overlap_mode == Overlap::Never)
                    .map(|icon| icon.collision(vertical));
                let label = VariableLabel { id, text, icon_feature, anchors: &anchors, run: &label };
                this.place_at_variable_anchors(&label, vertical, ctx)
            });
            // A label that didn't place fades from where it was.
            if !placed.result.placed
                && let Some(offset) = self.run.prev.as_ref().and_then(|prev| prev.variable_offsets.get(&id)).copied()
            {
                self.run.next.variable_offsets.insert(id, offset);
            }
            placed
        } else {
            return None;
        };
        if ctx.bucket.allow_vertical_placement
            && !placed.result.placed
            && let Some(prev) = self.run.prev.as_ref().and_then(|prev| prev.placed_orientations.get(&id)).copied()
        {
            self.run.next.placed_orientations.insert(id, prev);
        }
        Some(placed)
    }

    /// Places text with `place`, which takes whether to place it vertically,
    /// in each writing mode its bucket places in turn, until it places.
    fn place_in_writing_modes(
        &mut self,
        text: &TextPart,
        ctx: &PlacementContext<'_>,
        mut place: impl FnMut(&mut Self, bool) -> PlacedText,
    ) -> PlacedText {
        let bucket = ctx.bucket;
        if !bucket.allow_vertical_placement || !text.runs.writing_modes().has_vertical() {
            return place(self, false);
        }
        debug_assert_ne!(bucket.placement_modes.as_slice(), []);
        let mut placed = PlacedText::default();
        for &mode in &bucket.placement_modes {
            placed = place(self, mode == TextWritingModeType::Vertical);
            if placed.result.placed {
                return placed;
            }
        }
        PlacedText { vertical: false, ..placed }
    }

    /// `anchors` in the order to try them: a label placed before tries its
    /// anchor first, and in a tilted view that one alone.
    fn anchors_in_order(
        &self,
        id: NonZeroU32,
        anchors: Vec<SymbolAnchorType>,
        ctx: &PlacementContext<'_>,
    ) -> Vec<SymbolAnchorType> {
        let Some(prev_anchor) =
            self.run.prev.as_ref().and_then(|prev| prev.variable_offsets.get(&id)).map(|o| o.anchor)
        else {
            return anchors;
        };
        if anchors.first().is_none_or(|first| *first == prev_anchor) || !anchors.contains(&prev_anchor) {
            return anchors;
        }
        let rest = anchors.iter().copied().filter(|anchor| *anchor != prev_anchor);
        let tilted = ctx.view.pitch() != 0.0;
        std::iter::once(prev_anchor).chain(rest.filter(|_| !tilted)).collect()
    }

    /// Places `label`'s text, `vertical` or not, at the first of its
    /// anchors where it fits: first where it overlaps nothing, then where its
    /// overlap mode lets it overlap.
    fn place_at_variable_anchors(
        &mut self,
        label: &VariableLabel<'_>,
        vertical: bool,
        ctx: &PlacementContext<'_>,
    ) -> PlacedText {
        let VariableLabel { id, text, icon_feature, anchors, run: label } = *label;
        let pools = &ctx.bucket.state.symbol_instance_pools;
        let feature = text.collision(vertical);
        let unplaced = PlacedText { vertical, ..PlacedText::default() };
        let (Some(text_box), Some(offsets)) = (pools.boxes(feature).first(), &text.variable_anchors) else {
            return unplaced;
        };
        // The first pass tries each anchor where the text overlaps nothing,
        // and a second, unless it never overlaps, where its mode lets it.
        let passes: &[Overlap] = if ctx.text_overlap_mode == Overlap::Never {
            &[Overlap::Never]
        } else {
            &[Overlap::Never, ctx.text_overlap_mode]
        };
        let text_collision = ctx.text_collision();
        let icon_collision = ctx.icon_collision();
        // An anchor that doesn't fit leaves its result, which says whether
        // the text is hidden behind the planet.
        let mut last = PlacedFeatureResult::default();
        for &overlap_mode in passes {
            for &anchor in anchors {
                let offset = VariableOffset {
                    offset: offsets.get_offset_by_anchor(&anchor),
                    width: text_box.x2 - text_box.x1,
                    height: text_box.y2 - text_box.y1,
                    anchor,
                    text_box_scale: text.box_scale,
                };
                let shift = offset.layout_shift();
                let screen_shift = offset.screen_shift(ctx.rotate_text_with_map, ctx.pitch_text_with_map, ctx.bearing);
                self.run.text_boxes.clear();
                if !self.can_place_at_variable_anchor(text_box, anchor, screen_shift, anchors, ctx) {
                    continue;
                }
                let part = PartCollision { overlap_mode, ..text_collision };
                let result = collision::place_feature(
                    &*self.index,
                    &part,
                    feature,
                    pools.boxes(feature),
                    label,
                    Some(shift),
                    &mut self.run.text_boxes,
                );
                last = PlacedFeatureResult { placed: false, ..result };
                if let Some(icon_feature) = icon_feature {
                    let icon = collision::place_feature(
                        &*self.index,
                        &icon_collision,
                        icon_feature,
                        pools.boxes(icon_feature),
                        label,
                        Some(shift),
                        &mut self.run.icon_boxes,
                    );
                    self.run.icon_boxes.clear();
                    if !icon.placed {
                        continue;
                    }
                }
                if result.placed {
                    self.run.next.variable_offsets.entry(id).or_insert(offset);
                    if ctx.bucket.allow_vertical_placement {
                        self.run.next.placed_orientations.entry(id).or_insert(orientation(vertical));
                    }
                    return PlacedText { result, vertical, shift: Some(shift) };
                }
            }
        }
        PlacedText { result: last, ..unplaced }
    }

    /// Places `icon` beside `text`, which it follows to its variable anchor
    /// where it is fitted to it, and to its vertical box where it has one.
    /// Returns where, and whether it took the vertical box.
    fn place_icon(
        &mut self,
        icon: &IconPart,
        text: Option<PlacedText>,
        ctx: &PlacementContext<'_>,
    ) -> (PlacedFeatureResult, bool) {
        let state = &ctx.bucket.state;
        let shift = text.filter(|text| ctx.has_icon_text_fit && text.result.placed).and_then(|text| text.shift);
        let buffer = if icon.sdf { &state.sdf_icon } else { &state.icon };
        let run = &buffer.placed_symbols[icon.run as usize];
        let label =
            LabelRun { symbol: run, pools: &buffer.pools, font_size: evaluate_size_for_feature(&ctx.icon_size, run) };
        let vertical = text.is_some_and(|text| text.vertical) && icon.vertical_collision.is_some();
        let feature = icon.collision(vertical);
        let result = collision::place_feature(
            &*self.index,
            &ctx.icon_collision(),
            feature,
            state.symbol_instance_pools.boxes(feature),
            &label,
            shift,
            &mut self.run.icon_boxes,
        );
        (result, vertical)
    }
}
