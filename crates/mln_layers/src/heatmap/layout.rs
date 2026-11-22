use mln_render::layer::{BuildCx, GpuVec, Indexes, Layout, LayoutCx};
use mln_util::error::Error;

use super::Heatmap;
use super::bucket::HeatmapBucket;
use crate::points;

/// The features a tile's heat draws, by index.
pub struct HeatmapLayout(Vec<usize>);

impl Layout<Heatmap> for HeatmapLayout {
    type Bucket = HeatmapBucket;

    fn new(cx: &mut LayoutCx<'_, Heatmap>) -> Result<Self, Error> {
        Ok(HeatmapLayout(cx.selected()?))
    }

    /// The quads of the points in the tile: a point outside it is its
    /// neighbor's.
    fn build(self, cx: &mut BuildCx<'_, Heatmap>) -> Result<Option<HeatmapBucket>, Error> {
        let mut bucket =
            HeatmapBucket { corners: GpuVec::default(), triangles: Indexes::default(), binders: cx.binders() };
        for i in self.0 {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            let corners = bucket.corners.get_mut();
            points::add_quads(
                corners,
                &mut bucket.triangles,
                geometry.iter().flatten().copied().filter(|&p| points::in_tile(p)),
            );
            cx.populate(&mut bucket.binders, i, &feature, corners.len());
            cx.index(i, &geometry);
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}
