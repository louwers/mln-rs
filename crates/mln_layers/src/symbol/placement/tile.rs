//! Tile mode, where each tile is placed alone: the symbols that cross the
//! tile's borders, or its neighbors', place first, so the tiles on either
//! side of a border agree on them.

use glam::{DMat4, Vec2};
use mln_map_types::MapMode;
use mln_render::placement::{CollisionIndex, Overlap, PlaceCx, PlacementRecord, PlacingTile, Shape};
use mln_style_types::{SymbolAnchorType, SymbolPlacementType};
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::EXTENT;

use super::context::BucketContext;
use super::{JointPlacement, LayerBuckets, Placement, VariableOffset};
use crate::symbol::bucket::SymbolBucket;
use crate::symbol::collision_feature::CollisionBox;
use crate::symbol::instance::SymbolInstance;
use crate::symbol::placement::collision::{
    CollisionBoundaries, IntersectStatus, intersects_tile_edges, project_tile_boundaries,
};

/// Where a tile-mode placement is in its rounds.
#[derive(Clone, Copy)]
pub(crate) struct TileRounds {
    /// Placing the symbols that cross the borders, before the rest.
    pub(super) crossing: bool,
    /// The priority of the bucket whose crossing symbols place: lower first.
    priority: usize,
    /// The priority of the next bucket whose symbols may cross, across the
    /// layers.
    next_priority: usize,
}

impl Default for TileRounds {
    /// The rounds of a placement before it places, which places the
    /// symbols that cross the borders first.
    fn default() -> Self {
        TileRounds { crossing: true, priority: 0, next_priority: 0 }
    }
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
    /// Whether the placement places the symbols that cross tile borders.
    fn crossing(&self) -> bool {
        self.mode == MapMode::Tile && self.tile.crossing
    }

    /// The borders of the tile whose matrix is `matrix`, in tile mode,
    /// and those that `bucket`'s symbols stay inside, where it avoids its
    /// tile's edges or places along lines.
    pub(super) fn tile_borders(
        &self,
        bucket: &SymbolBucket,
        index: &CollisionIndex,
        matrix: &DMat4,
    ) -> (Option<CollisionBoundaries>, Option<CollisionBoundaries>) {
        if self.mode != MapMode::Tile {
            return (None, None);
        }
        let borders = project_tile_boundaries(index, matrix);
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
        ctx: &BucketContext<'_>,
        index: &CollisionIndex,
    ) -> bool {
        if self.mode != MapMode::Tile {
            return true;
        }
        let Some(borders) = &ctx.tile_borders else {
            return true;
        };
        let intersects = |shift| {
            intersects_tile_edges(
                index,
                ctx.view,
                text_box,
                shift,
                &ctx.tile.projection.main_matrix,
                ctx.pixel_ratio,
                borders,
            )
            .flags
                != IntersectStatus::NONE
        };
        if self.tile.crossing {
            if anchors.first() == Some(&anchor) && intersects(Vec2::ZERO) {
                return true;
            }
            // The most important labels try the other anchors now; the rest
            // wait for the second round.
            if self.tile.priority > 0 {
                return false;
            }
        }
        !intersects(shift)
    }

    /// Whether a symbol crossing the borders that didn't place places again
    /// with the rest, to try its other variable anchors.
    fn should_retry_placement(&self, placement: JointPlacement, ctx: &BucketContext<'_>) -> bool {
        self.crossing() && !placement.placed() && ctx.bucket.has_variable_text_anchors()
    }

    /// Adds to `records` the parts of `symbol`, placed as `placement`, in
    /// tile mode: for a point symbol that won't be placed again at another
    /// anchor, a record of its text, then one of its icon. `padding` is the
    /// collision index's.
    pub(super) fn symbol_placed(
        &mut self,
        symbol: &SymbolInstance,
        ctx: &BucketContext<'_>,
        placement: JointPlacement,
        padding: f32,
        records: Option<&mut Vec<PlacementRecord>>,
    ) {
        let Some(records) = records.filter(|_| self.mode == MapMode::Tile) else {
            return;
        };
        if ctx.along_line() || self.should_retry_placement(placement, ctx) {
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
        let crosses_border = !placement.skip_fade && self.tile.crossing;
        let parts = [
            ("text", first_box(&self.text_boxes), placement.text),
            ("icon", first_box(&self.icon_boxes), placement.icon),
        ];
        for (part, bounds, placed) in parts {
            let Some(bounds) = bounds else { continue };
            records.push(PlacementRecord {
                layer: ctx.bucket.bucket_leader_id.to_string(),
                key: String::from_utf8_lossy(ctx.bucket.state.symbol_instance_pools.key(symbol)).into_owned(),
                part,
                bounds,
                padding,
                placed,
                crosses_border,
            });
        }
    }

    /// Places the symbols of `layer`'s buckets on `tiles` that cross their
    /// tile's borders or a neighbor's into `cx`'s index: by bucket, and then
    /// by how they cross.
    pub(super) fn place_crossing(&mut self, layer: &LayerBuckets, tiles: &[PlacingTile], cx: &mut PlaceCx<'_>) {
        let PlaceCx { index, view, buckets, records, .. } = cx;
        let mut contexts = Vec::new();
        let mut intersections = Vec::new();
        for placed in &layer.placed {
            let tile = &tiles[placed.tile];
            if let Some(bucket) = buckets.get::<SymbolBucket>(&tile.bucket) {
                let ctx = BucketContext::new(self, layer, placed, tile, bucket, view, index);
                let before = intersections.len();
                self.intersections(&ctx, index, contexts.len(), &mut intersections);
                if intersections.len() > before {
                    contexts.push(ctx);
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

        for intersection in &intersections {
            let ctx = &contexts[intersection.ctx];
            self.tile.priority = intersection.priority;
            let symbol = &ctx.bucket.state.symbol_instances[intersection.symbol];
            if symbol.cross_tile_id.is_some_and(|id| self.seen.contains(&id)) {
                continue;
            }
            let placement = self.place_symbol(intersection.symbol, symbol, ctx, index, records.as_deref_mut());
            if !self.should_retry_placement(placement, ctx)
                && let Some(id) = symbol.cross_tile_id
            {
                self.seen.insert(id);
            }
        }
    }

    /// Adds to `found` the point symbols of `ctx`'s bucket, which is the
    /// placement's context `ctx_index`, that cross its tile's borders or a
    /// neighbor's. Each bucket of point symbols that may cross takes the
    /// next priority.
    fn intersections(
        &mut self,
        ctx: &BucketContext<'_>,
        index: &CollisionIndex,
        ctx_index: usize,
        found: &mut Vec<Intersection>,
    ) {
        if ctx.along_line() || *ctx.layout().symbol_avoid_edges.uniform() || self.mode != MapMode::Tile {
            return;
        }
        let priority = self.tile.next_priority;
        self.tile.next_priority += 1;
        let Some(tile_borders) = ctx.tile_borders else {
            return;
        };
        let render_tile = ctx.tile;
        let canonical = render_tile.id.canonical;
        let neighbor = |dx: i64, dy: i64| {
            let id = UnwrappedTileID::new(canonical.z, i64::from(canonical.x) + dx, i64::from(canonical.y) + dy);
            let matrix = ctx.view.projection_data(&id).fallback_matrix;
            Neighbor {
                shift: Vec2::new(-dx as f32, -dy as f32) * EXTENT as f32,
                matrix,
                borders: project_tile_boundaries(index, &matrix),
            }
        };
        let neighbors = [neighbor(0, -1), neighbor(0, 1), neighbor(-1, 0), neighbor(1, 0)];
        let crossing = |collision_box: &CollisionBox, shift: Vec2| {
            let own = intersects_tile_edges(
                index,
                ctx.view,
                collision_box,
                shift,
                &render_tile.projection.main_matrix,
                ctx.pixel_ratio,
                &tile_borders,
            );
            std::iter::once(own)
                .chain(neighbors.iter().map(|neighbor| {
                    intersects_tile_edges(
                        index,
                        ctx.view,
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
        let variable_icon = ctx.has_icon_text_fit && ctx.icon_overlap == Overlap::Never;
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

        let mut order = std::mem::take(&mut self.order);
        ctx.bucket_symbols(&mut order);
        found.extend(order.iter().filter_map(|&symbol_index| {
            let symbol = &ctx.bucket.state.symbol_instances[symbol_index];
            let status = symbol_crossing(symbol);
            (status.flags != IntersectStatus::NONE).then_some(Intersection {
                ctx: ctx_index,
                symbol: symbol_index,
                anchor_point: symbol.anchor.point,
                status,
                priority,
            })
        }));
        self.order = order;
    }
}
