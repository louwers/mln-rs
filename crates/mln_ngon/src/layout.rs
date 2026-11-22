use mln_rs::layer::{
    Bucket, BuildCx, Error, GpuVec, Indexes, LayerBinders, Layout, LayoutCx, MapMode, PaintPropertyBinder, Upload, Wire,
};
use mln_rs::mln_util::Color;
use mln_rs::mln_util::constants::EXTENT;

use crate::{Ngon, paint};

mln_rs::layer::paint_property_binders!(
    pub(crate) struct NgonBinders for paint::PossiblyEvaluated {
        ngon_color: PaintPropertyBinder<Color> => ["color"],
        ngon_radius: PaintPropertyBinder<f32> => ["radius"],
        ngon_corners: PaintPropertyBinder<f32> => ["corners"],
        ngon_rotate: PaintPropertyBinder<f32> => ["rotate"],
        ngon_blur: PaintPropertyBinder<f32> => ["blur"],
        ngon_opacity: PaintPropertyBinder<f32> => ["opacity"],
        ngon_stroke_color: PaintPropertyBinder<Color> => ["stroke_color"],
        ngon_stroke_width: PaintPropertyBinder<f32> => ["stroke_width"],
        ngon_stroke_opacity: PaintPropertyBinder<f32> => ["stroke_opacity"],
    }
);

/// A tile's polygons: a quad per point, each corner the point doubled with
/// the corner's right and bottom sides in the low bits.
#[derive(Clone, Wire)]
#[wire(crate = mln_rs::mln_wire)]
pub struct NgonBucket {
    pub(crate) corners: GpuVec<[i16; 2]>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<NgonBinders>,
}

impl Bucket for NgonBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}

/// The features the filter selects, by index.
pub struct NgonLayout(Vec<usize>);

impl Layout<Ngon> for NgonLayout {
    type Bucket = NgonBucket;

    fn new(cx: &mut LayoutCx<'_, Ngon>) -> Result<Self, Error> {
        Ok(NgonLayout(cx.selected()?))
    }

    fn build(self, cx: &mut BuildCx<'_, Ngon>) -> Result<Option<NgonBucket>, Error> {
        let mut bucket =
            NgonBucket { corners: GpuVec::default(), triangles: Indexes::default(), binders: cx.binders() };
        // A tile draws the points of its own square, so that a point in two
        // tiles' buffers draws once, but for a map of one tile, which draws
        // those of its buffer too.
        let buffered = cx.mode() == MapMode::Tile;
        for i in self.0 {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            let corners = bucket.corners.get_mut();
            for p in geometry.iter().flatten().filter(|&&p| buffered || in_tile(p.x, p.y)) {
                let (x, y, first) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2), corners.len());
                corners.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
                bucket.triangles.add(corners, first, [[0, 1, 2], [0, 3, 2]]);
            }
            cx.populate(&mut bucket.binders, i, &feature, corners.len());
            cx.index(i, &geometry);
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}

/// Whether the point `(x, y)` of a tile is in the tile's half-open square,
/// rather than in its buffer.
fn in_tile(x: i16, y: i16) -> bool {
    (0..EXTENT).contains(&i32::from(x)) && (0..EXTENT).contains(&i32::from(y))
}
