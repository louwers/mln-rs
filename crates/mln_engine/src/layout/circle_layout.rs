// The layout keeps the indices (and sort keys) of the features its filter
// accepted and gets the features again from the source layer when it
// builds the bucket.

use geometry::Point;
use mln_style::StyleValue as _;
use mln_style::constants::EXTENT;
use mln_style::error::Error;
use mln_style::expression::EvaluationContext;
use mln_style::layers::{Circle, CircleLayoutProperties};

use crate::buckets::circle_bucket::{CircleBucket, CircleLayoutVertex};
use crate::draw::mesh::{Indexed, add_point_quads};
use crate::layer::layout::{BuildCx, Layout, LayoutCx};
use crate::map::MapMode;

/// A feature the filter accepted, by index (see the file comment).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CircleFeature {
    pub(crate) i: usize,
    pub(crate) sort_key: f32,
}

/// The features a tile's circles draw, in `circle-sort-key` order.
pub(crate) struct CircleLayout {
    pub(crate) features: Vec<CircleFeature>,
}

impl Layout<Circle> for CircleLayout {
    type Bucket = CircleBucket;

    fn new(cx: &mut LayoutCx<'_, Circle>) -> Result<Self, Error> {
        let zoom = cx.zoom();
        let unevaluated_layout = &cx.layers()[0].style.layout;
        let sort_features_by_key = !unevaluated_layout.circle_sort_key.is_undefined();
        let layout = unevaluated_layout.evaluate(&cx.parameters());
        let sort_key_property = &layout.circle_sort_key;
        let constant_sort_key = if sort_features_by_key { sort_key_property.constant() } else { None };

        let mut features: Vec<CircleFeature> = Vec::new();
        let tile_layer = cx.source_layer()?;
        let context = cx.filter_context();
        let selection = cx.select(&tile_layer, context);
        for i in 0..tile_layer.len() {
            if !selection.may_select(i) {
                continue;
            }
            let feature = tile_layer.feature(i)?;
            if !selection.selects(&feature) {
                continue;
            }
            if !sort_features_by_key {
                features.push(CircleFeature { i, sort_key: CircleLayoutProperties::Defaults::circle_sort_key() });
                continue;
            }

            if let Some(sort_key) = constant_sort_key {
                features.push(CircleFeature { i, sort_key });
                continue;
            }

            let sort_key = sort_key_property.evaluate(&EvaluationContext::with_zoom_feature(zoom, Some(&feature)));
            let circle_feature = CircleFeature { i, sort_key };
            // std::lower_bound: the first feature whose key is not less.
            let sort_position = features.partition_point(|f| f.sort_key < sort_key);
            features.insert(sort_position, circle_feature);
        }
        if constant_sort_key.is_some() {
            // Inserting at the lower bound puts each feature before the
            // earlier ones with an equal key: keep that order without a sort.
            features.reverse();
        }

        Ok(CircleLayout { features })
    }

    fn build(self, cx: &mut BuildCx<'_, Circle>) -> Result<Option<CircleBucket>, Error> {
        let source_layer = cx.source_layer()?;
        let mut bucket = CircleBucket { paint_property_binders: cx.binders(), ..CircleBucket::default() };
        let mode = cx.mode();

        for circle_feature in &self.features {
            let i = circle_feature.i;
            let feature = source_layer.feature(i)?;
            let geometry = feature.geometry()?;

            // A map of many tiles leaves the points outside the tile to the
            // tiles they're in, so that a circle that several tiles hold in
            // their buffers draws once. A tile image draws them all, so that
            // circles from a neighbouring tile aren't cut off at its edge.
            let in_tile =
                |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
            let points = geometry.iter().flatten().filter(|p| mode == MapMode::Tile || in_tile(p)).copied();
            let mut triangles = Indexed { indexes: bucket.triangles.get_mut(), segments: &mut bucket.segments };
            add_point_quads(bucket.vertices.get_mut(), &mut triangles, points, |a1| CircleLayoutVertex { a1 });
            cx.populate(&mut bucket.paint_property_binders, i, &feature, bucket.vertices.len(), None, None);

            cx.index(i, &geometry);
        }

        Ok((!bucket.segments.is_empty()).then_some(bucket))
    }
}
