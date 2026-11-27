// As in circle_layout.rs, the layout keeps the indices of the features its
// filter accepted and gets the features again from the source layer when it
// builds the bucket.

use mln_engine::layer::{BuildCx, Layout, LayoutCx};
use mln_style::error::Error;
use mln_style::layers::Heatmap;

use crate::layers::heatmap::bucket::HeatmapBucket;

pub(crate) struct HeatmapLayout {
    /// The indices of the features the filter accepted, in order.
    features: Vec<usize>,
}

impl Layout<Heatmap> for HeatmapLayout {
    type Bucket = HeatmapBucket;

    fn new(cx: &mut LayoutCx<'_, Heatmap>) -> Result<Self, Error> {
        let tile_layer = cx.source_layer()?;
        let context = cx.filter_context();
        let selection = cx.select(&tile_layer, context);
        Ok(HeatmapLayout { features: selection.indexes(&tile_layer)? })
    }

    fn build(self, cx: &mut BuildCx<'_, Heatmap>) -> Result<Option<HeatmapBucket>, Error> {
        let source_layer = cx.source_layer()?;
        let mut bucket = HeatmapBucket { paint_property_binders: cx.binders(), ..HeatmapBucket::default() };

        for &i in &self.features {
            let feature = source_layer.feature(i)?;
            let geometry = feature.geometry()?;
            bucket.add_points(&geometry);
            cx.populate(&mut bucket.paint_property_binders, i, &feature, bucket.vertices.len(), None, None);
            cx.index(i, &geometry);
        }
        Ok((!bucket.segments.is_empty()).then_some(bucket))
    }
}
