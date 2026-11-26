//! Tile mode, where each tile is placed alone: the symbols that cross the
//! tile's borders, or its neighbors', place first, so the tiles on either
//! side of a border agree on them.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use glam::{DMat4, Vec2};
use mln_style::constants::EXTENT;
use mln_style::hash::HashSet;
use mln_style::types::{SymbolAnchorType, SymbolPlacementType};
use mln_tile_id::UnwrappedTileID;

use super::context::PlacementContext;
use super::{JointPlacement, PlacedSymbolData, Placement, PlacementInputs, PlacementKind, VariableOffset};
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::layer::render_layer::{BucketKey, SymbolBuckets};
use crate::layout::symbol_instance::SymbolInstance;
use crate::placement::collision_feature::CollisionBox;
use crate::placement::collision_grid::Shape;
use crate::placement::collision_index::{CollisionBoundaries, IntersectStatus, OverlapMode};

/// What a tile placement keeps besides the placement's own state.
#[derive(Default)]
pub(crate) struct TilePlacementState {
    pub(super) placed_symbols_data: Vec<PlacedSymbolData>,
    /// Placing the symbols that cross the borders, before the rest.
    populate_intersections: bool,
    /// The priority of the bucket whose crossing symbols place: lower first.
    current_intersection_priority: usize,
    pub(super) collect_data: bool,
}

/// A symbol that crosses its tile's borders, or a neighbor's.
struct Intersection {
    /// The index of the symbol's bucket's context among the placement's.
    ctx: usize,
    symbol: usize,
    anchor_point: Vec2,
    status: IntersectStatus,
    /// Lower places first.
    priority: usize,
}

/// A neighbor of a tile: where the tile's symbols are in it, and its
/// borders.
struct Neighbor {
    shift: Vec2,
    matrix: DMat4,
    borders: CollisionBoundaries,
}

impl Placement {
    fn tile_state(&mut self) -> Option<&mut TilePlacementState> {
        match &mut self.kind {
            PlacementKind::Tile(tile) => Some(tile),
            _ => None,
        }
    }

    /// The borders of the tile whose matrix is `matrix`, in tile mode,
    /// and those that `bucket`'s symbols stay inside, where it avoids its
    /// tile's edges or places along lines.
    pub(super) fn tile_borders(
        &self,
        bucket: &SymbolBucket,
        matrix: &DMat4,
    ) -> (Option<CollisionBoundaries>, Option<CollisionBoundaries>) {
        if !matches!(self.kind, PlacementKind::Tile(_)) {
            return (None, None);
        }
        let borders = self.collision_index.project_tile_boundaries(matrix);
        let layout = &*bucket.layout;
        let avoided =
            *layout.symbol_avoid_edges.uniform() || *layout.symbol_placement.uniform() == SymbolPlacementType::Line;
        (Some(borders), avoided.then_some(borders))
    }

    /// Whether a label at the variable anchor `anchor` of `anchors`, whose
    /// box is `text_box` moved by `shift`, may show: always outside tile
    /// mode; in tile mode where it stays inside its tile's borders, or, at
    /// the first anchor while crossing symbols place, where it crosses them
    /// unshifted, so a shift larger than the tile's buffer cuts nothing off.
    pub(super) fn can_place_at_variable_anchor(
        &self,
        text_box: &CollisionBox,
        anchor: SymbolAnchorType,
        shift: Vec2,
        anchors: &[SymbolAnchorType],
        ctx: &PlacementContext<'_>,
    ) -> bool {
        let PlacementKind::Tile(tile) = &self.kind else {
            return true;
        };
        let Some(borders) = &ctx.tile_borders else {
            return true;
        };
        let intersects = |shift| {
            self.collision_index
                .intersects_tile_edges(text_box, shift, &ctx.tile().matrix, ctx.pixel_ratio, borders)
                .flags
                != IntersectStatus::NONE
        };
        if tile.populate_intersections {
            if anchors.first() == Some(&anchor) && intersects(Vec2::ZERO) {
                return true;
            }
            // The most important labels try the other anchors now; the rest
            // wait for the second round.
            if tile.current_intersection_priority > 0 {
                return false;
            }
        }
        !intersects(shift)
    }

    /// Whether a symbol crossing the borders that didn't place places again
    /// with the rest, to try its other variable anchors.
    fn should_retry_placement(&self, placement: JointPlacement, ctx: &PlacementContext<'_>) -> bool {
        matches!(&self.kind, PlacementKind::Tile(tile) if tile.populate_intersections)
            && !placement.placed()
            && ctx.bucket.has_variable_text_anchors()
    }

    /// Records the data of `symbol`, placed as `placement`, where tile mode
    /// collects it: for a point symbol that won't be placed again at
    /// another anchor.
    pub(super) fn symbol_placed(
        &mut self,
        symbol: &SymbolInstance,
        ctx: &PlacementContext<'_>,
        placement: JointPlacement,
    ) {
        let viewport_padding = self.collision_index.viewport_padding();
        let should_retry = self.should_retry_placement(placement, ctx);
        let PlacementKind::Tile(tile) = &mut self.kind else {
            return;
        };
        if !tile.collect_data || ctx.along_line() || should_retry {
            return;
        }
        // A point symbol's part has one box.
        let first_box = |shapes: &[Option<Shape>]| {
            debug_assert!(shapes.len() <= 1);
            match shapes.first() {
                Some(Some(Shape::Box(aabb))) => Some(*aabb),
                _ => None,
            }
        };
        let text_collision_box = first_box(&self.text_boxes);
        let icon_collision_box = first_box(&self.icon_boxes);
        tile.placed_symbols_data.push(PlacedSymbolData {
            key: String::from_utf8_lossy(ctx.bucket.state.symbol_instance_pools.key(symbol)).into_owned(),
            text_collision_box,
            icon_collision_box,
            text_placed: placement.text,
            icon_placed: placement.icon,
            intersects_tile_border: !placement.skip_fade && tile.populate_intersections,
            viewport_padding,
            layer: ctx.bucket.bucket_leader_id.to_string(),
        });
    }

    /// Places `inputs`' buckets of `buckets` for a tile: the symbols that
    /// cross the borders first, by bucket and then by how they cross, and
    /// then the rest.
    pub(super) fn tile_place_buckets<B: SymbolBuckets + ?Sized>(
        &mut self,
        inputs: &PlacementInputs,
        buckets: &B,
        reloaded_placed: &mut HashSet<BucketKey>,
    ) {
        let Some(tile) = self.tile_state() else {
            return;
        };
        tile.placed_symbols_data.clear();
        tile.current_intersection_priority = 0;
        tile.populate_intersections = true;

        let mut contexts = Vec::new();
        let mut intersections = Vec::new();
        for data in inputs.buckets() {
            if let Some(bucket) = buckets.get(&data.bucket) {
                let ctx = self.context(inputs, data, bucket, reloaded_placed);
                let found = self.intersections(&ctx, contexts.len());
                if !found.is_empty() {
                    contexts.push(ctx);
                    intersections.extend(found);
                }
            }
        }
        // A stable sort: crossings that tie keep the order the buckets
        // found them in. Those crossing both kinds of border go first, then
        // vertical, then horizontal ones; then those cut off more.
        intersections.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(b.status.flags.cmp(&a.status.flags))
                .then(b.status.min_section_length.cmp(&a.status.min_section_length))
                .then(a.anchor_point.y.total_cmp(&b.anchor_point.y))
                .then(a.anchor_point.x.total_cmp(&b.anchor_point.x))
        });

        let mut seen_cross_tile_ids: BTreeSet<NonZeroU32> = BTreeSet::new();
        for intersection in &intersections {
            let ctx = &contexts[intersection.ctx];
            if let Some(tile) = self.tile_state() {
                tile.current_intersection_priority = intersection.priority;
            }
            let symbol = &ctx.bucket.state.symbol_instances[intersection.symbol];
            if symbol.cross_tile_id.is_some_and(|id| seen_cross_tile_ids.contains(&id)) {
                continue;
            }
            let placement = self.place_symbol(symbol, ctx);
            if !self.should_retry_placement(placement, ctx)
                && let Some(id) = symbol.cross_tile_id
            {
                seen_cross_tile_ids.insert(id);
            }
        }
        drop(contexts);

        if let Some(tile) = self.tile_state() {
            tile.populate_intersections = false;
        }
        for data in inputs.buckets() {
            if let Some(bucket) = buckets.get(&data.bucket) {
                let ctx = self.context(inputs, data, bucket, reloaded_placed);
                self.place_bucket(&ctx, &mut seen_cross_tile_ids, reloaded_placed);
            }
        }
    }

    /// The point symbols of `ctx`'s bucket, which is the placement's
    /// context `ctx_index`, that cross its tile's borders or a neighbor's.
    /// Each bucket of point symbols that may cross takes the next priority.
    fn intersections(&mut self, ctx: &PlacementContext<'_>, ctx_index: usize) -> Vec<Intersection> {
        let layout = ctx.layout();
        if ctx.along_line() || *layout.symbol_avoid_edges.uniform() {
            return Vec::new();
        }
        let Some(tile) = self.tile_state() else {
            return Vec::new();
        };
        let priority = tile.current_intersection_priority;
        tile.current_intersection_priority += 1;
        let Some(tile_borders) = ctx.tile_borders else {
            return Vec::new();
        };
        let index = &self.collision_index;
        let render_tile = ctx.tile();
        let canonical = render_tile.id.canonical;
        let neighbor = |dx: i64, dy: i64| {
            let id = UnwrappedTileID::new(canonical.z, i64::from(canonical.x) + dx, i64::from(canonical.y) + dy);
            let matrix = ctx.view.projection_data(&id).fallback_matrix;
            Neighbor {
                shift: Vec2::new(-dx as f32, -dy as f32) * EXTENT as f32,
                matrix,
                borders: index.project_tile_boundaries(&matrix),
            }
        };
        let neighbors = [neighbor(0, -1), neighbor(0, 1), neighbor(-1, 0), neighbor(1, 0)];
        let crossing = |collision_box: &CollisionBox, shift: Vec2| {
            let own =
                index.intersects_tile_edges(collision_box, shift, &render_tile.matrix, ctx.pixel_ratio, &tile_borders);
            std::iter::once(own)
                .chain(neighbors.iter().map(|neighbor| {
                    index.intersects_tile_edges(
                        collision_box,
                        shift + neighbor.shift,
                        &neighbor.matrix,
                        ctx.pixel_ratio,
                        &neighbor.borders,
                    )
                }))
                .find(|status| status.flags != IntersectStatus::NONE)
                .unwrap_or_default()
        };

        let pools = &ctx.bucket.state.symbol_instance_pools;
        let variable_icon = ctx.has_icon_text_fit && ctx.icon_overlap_mode == OverlapMode::Never;
        let symbol_crossing = |symbol: &SymbolInstance| {
            let text = symbol.text.as_ref();
            let first_anchor = text.and_then(|text| text.variable_anchors.as_ref()).and_then(|offsets| {
                offsets
                    .into_iter()
                    .next()
                    .map(|pair| (pair.anchor_type, offsets.get_offset_by_anchor(&pair.anchor_type)))
            });
            // Shifted to the first variable anchor, for the text and for an
            // icon fitted to it.
            let shift = |collision_box: &CollisionBox, text_box_scale: f32| {
                first_anchor.map_or(Vec2::ZERO, |(anchor, offset)| {
                    VariableOffset {
                        offset,
                        width: collision_box.x2 - collision_box.x1,
                        height: collision_box.y2 - collision_box.y1,
                        anchor,
                        text_box_scale,
                    }
                    .screen_shift(ctx.rotate_text_with_map, ctx.pitch_text_with_map, ctx.bearing)
                })
            };
            let mut status = IntersectStatus::default();
            if let Some(text) = text
                && let Some(text_box) = pools.boxes(&text.collision).first()
            {
                status = crossing(text_box, shift(text_box, text.box_scale));
            }
            if let Some(icon) = &symbol.icon
                && let Some(icon_box) = pools.boxes(&icon.collision).first()
            {
                let text_box_scale = text.map_or(0.0, |text| text.box_scale);
                let icon_shift = if variable_icon { shift(icon_box, text_box_scale) } else { Vec2::ZERO };
                let icon_status = crossing(icon_box, icon_shift);
                status.flags |= icon_status.flags;
                status.min_section_length = status.min_section_length.max(icon_status.min_section_length);
            }
            status
        };

        ctx.bucket_symbols()
            .into_iter()
            .filter_map(|symbol_index| {
                let symbol = &ctx.bucket.state.symbol_instances[symbol_index];
                let status = symbol_crossing(symbol);
                (status.flags != IntersectStatus::NONE).then_some(Intersection {
                    ctx: ctx_index,
                    symbol: symbol_index,
                    anchor_point: symbol.anchor.point,
                    status,
                    priority,
                })
            })
            .collect()
    }
}
