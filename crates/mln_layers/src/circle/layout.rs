use mln_map_types::MapMode;
use mln_render::layer::{BuildCx, GpuVec, Indexes, Layout, LayoutCx};
use mln_style::StyleValue as _;
use mln_style::expression::EvaluationContext;
use mln_util::error::Error;

use super::Circle;
use super::bucket::CircleBucket;
use crate::{features, points};

/// The features a tile's circles draw, by index, in `circle-sort-key`
/// order.
pub struct CircleLayout(Vec<usize>);

impl Layout<Circle> for CircleLayout {
    type Bucket = CircleBucket;

    fn new(cx: &mut LayoutCx<'_, Circle>) -> Result<Self, Error> {
        let layout = &cx.layers()[0].style.layout;
        let key = (!layout.circle_sort_key.is_undefined()).then(|| layout.evaluate(cx.parameters()).circle_sort_key);
        let zoom = f32::from(cx.tile().overscaled_z);
        let selected = cx.selected()?.into_iter().map(|i| (i, ())).collect();
        let features = features::sort(selected, key.as_ref(), |key, i| {
            Ok(key.evaluate(&EvaluationContext::with_zoom_feature(zoom, Some(&cx.feature(i)?))))
        })?;
        Ok(CircleLayout(features.into_iter().map(|(i, ())| i).collect()))
    }

    fn build(self, cx: &mut BuildCx<'_, Circle>) -> Result<Option<CircleBucket>, Error> {
        let mut bucket =
            CircleBucket { corners: GpuVec::default(), triangles: Indexes::default(), binders: cx.binders() };
        // A continuous map leaves the points outside the tile to the tiles
        // they're in. The others draw them all, so that circles from a
        // neighboring tile aren't cut at the tile's edge.
        let continuous = cx.mode() == MapMode::Continuous;
        for i in self.0 {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            let corners = bucket.corners.get_mut();
            let drawn = geometry.iter().flatten().copied().filter(|&p| !continuous || points::in_tile(p));
            points::add_quads(corners, &mut bucket.triangles, drawn);
            cx.populate(&mut bucket.binders, i, &feature, corners.len(), None, None);
            cx.index(i, &geometry);
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}
