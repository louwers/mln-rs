use std::mem;

use mln_rs::glam::DVec2;
use mln_rs::layer::placement::{
    Aabb, Item, Opacity, Overlap, PlaceCx, Placement, PlacementCx, PlacementRecord, PlacingLayer, Prepared, Shape,
};
use mln_rs::layer::{LayerStyle, MapMode, PropertyValue, TileProjector, TranslateAnchorType};
use mln_rs::mln_style_types::CirclePitchScaleType;
use mln_rs::mln_util::constants::EXTENT;

use crate::bucket::NgonBucket;
use crate::layer::{NgonLayer, outer_radius, transform};
use crate::{Ngon, layout, paint};

/// Places the polygons of the `org.maplibre.ngon` layers that collide
/// ([`layout`]): each shows where its circle, around its stroke's corners,
/// meets nothing placed before it.
#[derive(Default)]
pub struct NgonPlacement {
    /// What sized and moved each layer's circles in the current placement,
    /// and in the frame's.
    placed: Vec<Circles>,
    next: Vec<Circles>,
}

/// The paint values of a layer that size and move its circles.
#[derive(Clone, Copy, PartialEq)]
struct Circles {
    size: Option<f32>,
    translate: [f32; 2],
    anchor: TranslateAnchorType,
    scale: CirclePitchScaleType,
}

impl Placement for NgonPlacement {
    type Layer = NgonLayer;

    /// Whether a layer's polygons collide: unless both its layout
    /// properties are `true`.
    fn places(style: &LayerStyle<Ngon>) -> bool {
        let free =
            |value: &PropertyValue<bool>| matches!(value, PropertyValue::Undefined | PropertyValue::Constant(true));
        let layout::Unevaluated { ngon_allow_overlap, ngon_ignore_placement } = &style.layout;
        !(free(ngon_allow_overlap) && free(ngon_ignore_placement))
    }

    fn prepare(&mut self, cx: &mut PlacementCx<'_, NgonLayer>) -> Prepared {
        self.next.clear();
        self.next.extend(cx.layers.iter().map(|layer| {
            let paint = &layer.layer.evaluated.paint;
            Circles {
                size: constant_size(paint),
                translate: *paint.ngon_translate.uniform(),
                anchor: *paint.ngon_translate_anchor.uniform(),
                scale: *paint.ngon_pitch_scale.uniform(),
            }
        }));
        Prepared { added: false, changed: self.next != self.placed }
    }

    fn place_crossing(&mut self, layer: &PlacingLayer<'_, NgonLayer>, cx: &mut PlaceCx<'_>) {
        place(layer, cx, Some(true));
    }

    fn place(&mut self, layer: &PlacingLayer<'_, NgonLayer>, cx: &mut PlaceCx<'_>) {
        place(layer, cx, (cx.mode == MapMode::Tile).then_some(false));
    }

    fn commit(&mut self, _cx: &mut PlaceCx<'_>) {
        mem::swap(&mut self.placed, &mut self.next);
    }

    fn forget(&mut self, placement: bool) {
        if placement {
            self.placed.clear();
        }
    }
}

/// The distance from the center of each of a layer's polygons to its
/// stroke's corners, where no feature sizes them.
pub(crate) fn constant_size(paint: &paint::PossiblyEvaluated) -> Option<f32> {
    Some(outer_radius(
        paint.ngon_radius.constant()?,
        paint.ngon_stroke_width.constant()?,
        paint.ngon_corners.constant()?,
    ))
}

/// Places `layer`'s points: with `crossing`, those whose circle crosses
/// their tile's borders or those whose circle doesn't, and else all.
fn place(layer: &PlacingLayer<'_, NgonLayer>, cx: &mut PlaceCx<'_>, crossing: Option<bool>) {
    let (layout, paint) = (&layer.layer.style.layout, &layer.layer.evaluated.paint);
    let zoom = cx.view.zoom() as f32;
    let overlap = if layout.ngon_allow_overlap.evaluate_at(zoom, true) { Overlap::Always } else { Overlap::Never };
    let ignore_placement = layout.ngon_ignore_placement.evaluate_at(zoom, true);
    let size = constant_size(paint);
    let scale_with_map = *paint.ngon_pitch_scale.uniform() == CirclePitchScaleType::Map;
    let camera = cx.view.camera_to_center_distance();
    let how = transform(paint);
    for (t, tile) in layer.tiles.iter().enumerate() {
        let Some(bucket) = cx.buckets.get_mut::<NgonBucket>(&tile.bucket) else { continue };
        let projector = TileProjector::with_data(cx.view, &tile.id, tile.projection);
        let [dx, dy] = how.tile_units(&tile.id, cx.view);
        let project =
            |x: f32, y: f32| projector.project(&mln_rs::geometry::Point::new(f64::from(x), f64::from(y)), 0.0);
        let square = crossing.map(|_| {
            let corner = |at: f32| {
                let at = project(at, at).point;
                cx.index.to_grid(DVec2::new(at.x, at.y))
            };
            let (a, b) = (corner(0.0), corner(EXTENT as f32));
            Aabb::new(a.min(b), a.max(b))
        });
        // A continuous map's points fade on from where the placement before
        // left them; those of a bucket it didn't place show or hide at once.
        let laid = bucket.opacity.len();
        let placed = if cx.mode == MapMode::Continuous { laid } else { 0 };
        bucket.opacity.resize(bucket.points.len(), Opacity::new(false, true));
        for (i, point) in bucket.points.iter().enumerate() {
            let at = project(f32::from(point.at[0]) + dx, f32::from(point.at[1]) + dy);
            let center = cx.index.to_grid(DVec2::new(at.point.x, at.point.y));
            let distance = at.signed_distance_from_camera;
            let scale = if scale_with_map { (camera / distance) as f32 } else { 1.0 };
            let shape = Shape::Circle { center, radius: size.unwrap_or(point.size) * scale };
            let bounds = shape.bounds();
            let crosses = square.is_some_and(|square| crosses(&bounds, &square));
            if crossing.is_some_and(|crossing| crossing != crosses) {
                continue;
            }
            let shows = !tile.held_for_fade
                && !at.occluded
                && distance > 0.0
                && cx.index.in_grid(&bounds)
                && !cx.index.collides(&shape, overlap);
            if shows {
                let item = Item { tile: t as u32, feature: point.feature, sort: i as u32, overlap, ignore_placement };
                cx.index.insert(shape, item);
            }
            let prev = (i < placed).then(|| bucket.opacity[i]);
            let next = prev.map_or(Opacity::new(shows, true), |prev| prev.step(cx.fade, shows));
            cx.placed.add(prev, next);
            // The bucket draws its points shown until they first place, and
            // only a changed vector uploads again.
            let drawn = if i < laid { bucket.opacity[i] } else { Opacity::new(true, true) };
            if next.vertex() != drawn.vertex() {
                bucket.fade.get_mut()[4 * i..4 * i + 4].fill(next.vertex());
            }
            bucket.opacity[i] = next;
            if let Some(records) = &mut cx.records {
                records.push(PlacementRecord {
                    layer: layer.layer.id().to_owned(),
                    key: String::new(),
                    part: "ngon",
                    bounds,
                    placed: shows,
                    crosses_border: crosses,
                });
            }
        }
    }
}

/// Whether `bounds` reach across an edge of `square`.
fn crosses(bounds: &Aabb, square: &Aabb) -> bool {
    let meets = bounds.min.cmple(square.max).all() && square.min.cmple(bounds.max).all();
    let inside = square.min.cmple(bounds.min).all() && bounds.max.cmple(square.max).all();
    meets && !inside
}
