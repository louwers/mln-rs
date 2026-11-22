use geometry::Point;
use mln_map_types::MapMode;
use mln_render::layer::{BuildCx, GpuVec, Indexed, Layout, LayoutCx};
use mln_style::StyleValue as _;
use mln_style::expression::EvaluationContext;
use mln_util::constants::EXTENT;
use mln_util::error::Error;

use super::Circle;
use super::bucket::CircleBucket;

/// The features a tile's circles draw, by index, in `circle-sort-key`
/// order.
pub struct CircleLayout(Vec<usize>);

impl Layout<Circle> for CircleLayout {
    type Bucket = CircleBucket;

    fn new(cx: &mut LayoutCx<'_, Circle>) -> Result<Self, Error> {
        let style = &cx.layers()[0].style;
        let sorted = !style.layout.circle_sort_key.is_undefined();
        let sort_key = style.layout.evaluate(cx.parameters()).circle_sort_key;
        let constant = if sorted { sort_key.constant() } else { None };
        let zoom = f32::from(cx.tile().overscaled_z);
        let mut features: Vec<(f32, usize)> = Vec::new();
        for i in cx.selected()? {
            if sorted && constant.is_none() {
                let key = sort_key.evaluate(&EvaluationContext::with_zoom_feature(zoom, Some(&cx.feature(i)?)));
                // Before the first feature whose key isn't less.
                features.insert(features.partition_point(|&(k, _)| k < key), (key, i));
            } else {
                features.push((0.0, i));
            }
        }
        if constant.is_some() {
            // Each feature goes before the earlier ones of its key, as
            // inserting it would put it.
            features.reverse();
        }
        Ok(CircleLayout(features.into_iter().map(|(_, i)| i).collect()))
    }

    fn build(self, cx: &mut BuildCx<'_, Circle>) -> Result<Option<CircleBucket>, Error> {
        let mut bucket = CircleBucket {
            corners: GpuVec::default(),
            triangles: GpuVec::default(),
            segments: Vec::new(),
            binders: cx.binders(),
        };
        // A continuous map leaves the points outside the tile to the tiles
        // they're in. The others draw them all, so that circles from a
        // neighboring tile aren't cut at the tile's edge.
        let in_tile = |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
        let continuous = cx.mode() == MapMode::Continuous;
        for i in self.0 {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            let corners = bucket.corners.get_mut();
            let mut triangles = Indexed { indexes: bucket.triangles.get_mut(), segments: &mut bucket.segments };
            for p in geometry.iter().flatten().filter(|p| !continuous || in_tile(p)) {
                let (x, y) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2));
                let first = corners.len();
                corners.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
                triangles.add(corners, first, [[0, 1, 2], [0, 3, 2]]);
            }
            cx.populate(&mut bucket.binders, i, &feature, corners.len());
            cx.index(i, &geometry);
        }
        Ok((!bucket.segments.is_empty()).then_some(bucket))
    }
}
