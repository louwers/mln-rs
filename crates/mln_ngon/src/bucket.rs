// A tile's polygons: a quad per point, as circles' are. The layout keeps the
// indexes of the features the filter selects and gets them again from the
// source layer when it builds the bucket.

use geometry::Point;
use mln_engine::draw::DrawRange;
use mln_engine::draw::ground::point_center;
use mln_engine::layer::binders::{LayerBinders, PaintPropertyBinder};
use mln_engine::layer::{Bucket, BuildCx, EvaluatedLayer, GeometryVec, Layout, LayoutCx, Upload, Wire};
use mln_engine::map::MapMode;
use mln_style::color::Color;
use mln_style::constants::EXTENT;
use mln_style::error::Error;

use crate::{Ngon, paint};

mln_engine::paint_property_binders!(
    pub struct NgonBinders for paint::PossiblyEvaluated {
        ngon_radius: PaintPropertyBinder<f32> => ["radius"],
        ngon_corners: PaintPropertyBinder<f32> => ["corners"],
        ngon_rotate: PaintPropertyBinder<f32> => ["rotate"],
        ngon_color: PaintPropertyBinder<Color> => ["color"],
        ngon_blur: PaintPropertyBinder<f32> => ["blur"],
        ngon_opacity: PaintPropertyBinder<f32> => ["opacity"],
        ngon_stroke_width: PaintPropertyBinder<f32> => ["stroke_width"],
        ngon_stroke_color: PaintPropertyBinder<Color> => ["stroke_color"],
        ngon_stroke_opacity: PaintPropertyBinder<f32> => ["stroke_opacity"],
    }
);

/// The most quads a draw call draws, as many as its 16-bit indexes reach.
pub(crate) const RANGE_QUADS: usize = u16::MAX as usize / 4;

/// A tile's quads: each corner the point doubled, with the corner's right
/// and bottom sides in the low bits. A draw call draws [`RANGE_QUADS`] of
/// them ([`NgonBucket::ranges`]), whose indexes count from its first
/// vertex.
#[derive(Clone, Default, Wire)]
pub(crate) struct NgonBucket {
    pub(crate) vertices: GeometryVec<[i16; 2]>,
    /// The center of each vertex's point, which the upload keeps before the
    /// vertices move: where the heights of the ground under them are.
    #[wire(skip)]
    pub(crate) centers: Vec<[i16; 2]>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) binders: LayerBinders<NgonBinders>,
}

impl NgonBucket {
    /// The draw calls that draw the quads.
    pub(crate) fn ranges(&self) -> Vec<DrawRange> {
        let quads = self.vertices.len() / 4;
        (0..quads)
            .step_by(RANGE_QUADS)
            .map(|first| DrawRange {
                first_index: (6 * first) as u32,
                index_count: (6 * RANGE_QUADS.min(quads - first)) as u32,
                base_vertex: (4 * first) as i32,
                first_instance: 0,
                instance_count: 1,
            })
            .collect()
    }

    /// How far, in pixels, outside a point a query for `layer` still hits
    /// its polygon, from the layer's values and the bucket's statistics of
    /// them.
    pub(crate) fn query_radius(&self, layer: &EvaluatedLayer<Ngon>) -> f32 {
        let paint = &layer.paint;
        let binders = self.binders.get(&layer.style.id);
        let radius = binders
            .and_then(|b| b.ngon_radius.statistics.max())
            .or_else(|| paint.ngon_radius.constant())
            .unwrap_or_else(paint::Defaults::ngon_radius);
        let stroke = binders
            .and_then(|b| b.ngon_stroke_width.statistics.max())
            .or_else(|| paint.ngon_stroke_width.constant())
            .unwrap_or_else(paint::Defaults::ngon_stroke_width);
        let [x, y] = *paint.ngon_translate.uniform();
        // A stroke's corners are at most twice its width from the fill's,
        // a triangle's.
        radius.max(0.0) + 2.0 * stroke.max(0.0) + x.hypot(y)
    }
}

impl Bucket for NgonBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        if up.caps().is_some() && !self.vertices.is_moved() {
            self.centers = self.vertices.data().iter().map(|&vertex| point_center(vertex)).collect();
        }
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}

/// The features the filter selects, by index.
pub(crate) struct NgonLayout {
    features: Vec<usize>,
}

impl Layout<Ngon> for NgonLayout {
    type Bucket = NgonBucket;

    fn new(cx: &mut LayoutCx<'_, Ngon>) -> Result<Self, Error> {
        let source_layer = cx.source_layer()?;
        let context = cx.filter_context();
        let features = cx.select(&source_layer, context).indexes(&source_layer)?;
        Ok(NgonLayout { features })
    }

    fn build(self, cx: &mut BuildCx<'_, Ngon>) -> Result<Option<NgonBucket>, Error> {
        let source_layer = cx.source_layer()?;
        let mut bucket = NgonBucket { binders: cx.binders(), ..NgonBucket::default() };
        // A map of many tiles leaves the points outside the tile to the
        // tiles they're in, so that a polygon that several tiles hold in
        // their buffers draws once. A tile image draws them all, so that
        // polygons from a neighbouring tile aren't cut off at its edge.
        let all = cx.mode() == MapMode::Tile;
        let in_tile = |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
        for i in self.features {
            let feature = source_layer.feature(i)?;
            let geometry = feature.geometry()?;
            for p in geometry.iter().flatten().filter(|p| all || in_tile(p)) {
                let (x, y) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2));
                let vertices = bucket.vertices.get_mut();
                let first = (4 * (vertices.len() / 4 % RANGE_QUADS)) as u16;
                vertices.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
                bucket.triangles.get_mut().extend([0, 1, 2, 0, 3, 2].map(|i| first + i));
            }
            cx.populate(&mut bucket.binders, i, &feature, bucket.vertices.len(), None, None);
            cx.index(i, &geometry);
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}
