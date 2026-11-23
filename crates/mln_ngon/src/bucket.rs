use mln_rs::layer::placement::{Opacity, Placement};
use mln_rs::layer::{
    Bucket, BuildCx, Error, Evaluated, GpuVec, Indexes, LayerBinders, Layout, LayoutCx, MapMode, PaintPropertyBinder,
    Upload, Wire,
};
use mln_rs::mln_util::Color;
use mln_rs::mln_util::constants::EXTENT;

use crate::layer::outer_radius;
use crate::placement::{NgonPlacement, constant_size};
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
/// the corner's right and bottom sides in the low bits, and each corner's
/// opacity.
#[derive(Clone, Wire)]
#[wire(crate = mln_rs::mln_wire)]
pub struct NgonBucket {
    pub(crate) corners: GpuVec<[i16; 2]>,
    /// Each corner's [`Opacity::vertex`]: shown, where the layers don't
    /// place, and else as placement last left its point.
    pub(crate) fade: GpuVec<f32>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<NgonBinders>,
    /// The points, in quad order, where the layers place.
    pub(crate) points: Vec<NgonPoint>,
    /// Each point's opacity as placement last left it; empty before its
    /// first placement.
    #[wire(skip)]
    pub(crate) opacity: Vec<Opacity>,
}

/// A point that places.
#[derive(Clone, Copy, Wire)]
#[wire(crate = mln_rs::mln_wire)]
pub(crate) struct NgonPoint {
    /// Where it is in the tile.
    pub(crate) at: [i16; 2],
    /// Its feature's index in the tile's source layer.
    pub(crate) feature: u32,
    /// The distance from its center to its stroke's corners at the tile's
    /// zoom, in pixels, where a property that sizes it is data-driven.
    pub(crate) size: f32,
}

impl Bucket for NgonBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        // Placement rewrites the opacities of the points that place.
        if self.points.is_empty() {
            up.vec(&mut self.fade);
        } else {
            up.keep(&mut self.fade);
        }
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}

/// The features the filter selects, by index, whether the layers place,
/// and each feature's size where they do and it is data-driven.
pub struct NgonLayout {
    features: Vec<usize>,
    places: bool,
    sizes: Vec<f32>,
}

impl Layout<Ngon> for NgonLayout {
    type Bucket = NgonBucket;

    fn new(cx: &mut LayoutCx<'_, Ngon>) -> Result<Self, Error> {
        let features = cx.selected()?;
        let leader = &cx.layers()[0];
        let places = NgonPlacement::places(&leader.style);
        let paint = &leader.paint;
        let mut sizes = Vec::new();
        if places && constant_size(paint).is_none() {
            for &i in &features {
                let feature = cx.feature(i)?;
                let at = cx.context(&feature);
                let value = |value: &Evaluated<f32>| value.evaluate(&at);
                sizes.push(outer_radius(
                    value(&paint.ngon_radius),
                    value(&paint.ngon_stroke_width),
                    value(&paint.ngon_corners),
                ));
            }
        }
        Ok(NgonLayout { features, places, sizes })
    }

    fn build(self, cx: &mut BuildCx<'_, Ngon>) -> Result<Option<NgonBucket>, Error> {
        let mut bucket = NgonBucket {
            corners: GpuVec::default(),
            fade: GpuVec::default(),
            triangles: Indexes::default(),
            binders: cx.binders(),
            points: Vec::new(),
            opacity: Vec::new(),
        };
        // A tile draws the points of its own square, so that a point in two
        // tiles' buffers draws once, but for a map of one tile, which draws
        // those of its buffer too.
        let buffered = cx.mode() == MapMode::Tile;
        let shown = Opacity::new(true, true).vertex();
        for (n, i) in self.features.into_iter().enumerate() {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            let corners = bucket.corners.get_mut();
            for p in geometry.iter().flatten().filter(|&&p| buffered || in_tile(p.x, p.y)) {
                let (x, y, first) = (p.x.wrapping_mul(2), p.y.wrapping_mul(2), corners.len());
                corners.extend([[x, y], [x | 1, y], [x | 1, y | 1], [x, y | 1]]);
                bucket.triangles.add(corners, first, [[0, 1, 2], [0, 3, 2]]);
                bucket.fade.get_mut().extend([shown; 4]);
                if self.places {
                    let size = self.sizes.get(n).copied().unwrap_or_default();
                    bucket.points.push(NgonPoint { at: [p.x, p.y], feature: i as u32, size });
                }
            }
            cx.populate(&mut bucket.binders, i, &feature, corners.len());
            // Queries find the polygons that place through placement.
            if !self.places {
                cx.index(i, &geometry);
            }
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}

/// Whether the point `(x, y)` of a tile is in the tile's half-open square,
/// rather than in its buffer.
fn in_tile(x: i16, y: i16) -> bool {
    (0..EXTENT).contains(&i32::from(x)) && (0..EXTENT).contains(&i32::from(y))
}
