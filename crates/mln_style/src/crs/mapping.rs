//! How lng/lat maps to and from the world square of a registered projection.

use std::sync::{Arc, OnceLock};

use geometry::Point;
use smallvec::{SmallVec, smallvec};

use super::{Crs, TileMatrixSet};
use crate::geo::LatLngBounds;

/// A position no projection answers for.
const NO_ANSWER: [f64; 2] = [f64::NAN, f64::NAN];

/// How far `forward` may take an inverse from the position it came from, as
/// a fraction of the zoom 0 extent, for the inverse to count as that
/// position's answer: loose enough for a converter that is only nearly
/// invertible, tight enough to refuse an answer from the other side of the
/// world.
const ROUND_TRIP_TOLERANCE: f64 = 1e-3;

/// How many times the line to the center of the world square is halved in the
/// search for the last position with an answer: enough to place it within a
/// billionth of the line.
const EDGE_SEARCH_STEPS: usize = 30;

/// How many times as much as at the center of its tile matrix set a
/// projection may stretch distances along the meridian within its latitude
/// range.
const MAX_MERIDIAN_STRETCH: f64 = 10.0;

/// How many parts a lng/lat box is cut into along each axis when it is
/// sampled.
const BOX_SAMPLE_STEPS: usize = 16;

/// How many samples a side of a lng/lat box has, its corners included.
const BOX_SAMPLE_SIDE: usize = BOX_SAMPLE_STEPS + 1;

/// Positions for one call of a converter, inline for a single one, so that
/// converting one position allocates nothing.
type Positions = SmallVec<[[f64; 2]; 1]>;

fn is_answer(position: &[f64; 2]) -> bool {
    position[0].is_finite() && position[1].is_finite()
}

/// A rectangle in the world square, its sides along the axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBox {
    pub min: Point<f64>,
    pub max: Point<f64>,
}

/// A lng/lat box as places in the world square
/// ([`CrsMapping::lng_lat_box_samples`]).
#[derive(Clone, Debug, PartialEq)]
pub struct BoxSamples {
    /// The box's samples, row by row from its north-west corner.
    pub points: Vec<Point<f64>>,
    /// How far, in world units, the box's edges stray from the lines
    /// between neighboring samples.
    pub bulge: f64,
}

/// How lng/lat maps to and from the world square of a registered projection,
/// where tile 0/0/0 spans 0 to 1 on both axes: world x and y are the CRS's
/// coordinates from its tile matrix set's origin, over the extent at zoom 0,
/// with y growing down. The world is a bounded plane, with no world copies,
/// no wrapping and no latitude limit.
///
/// A place without an answer of its own falls back as GL JS's does: the
/// nearest place inside the square, then the edge of the answers on the line
/// to its center, then the converter's own finite answer.
#[derive(Debug)]
pub struct CrsMapping {
    crs: Arc<Crs>,
    /// The latitude range, found on its first use.
    latitude_range: OnceLock<[f64; 2]>,
}

impl CrsMapping {
    /// The mapping of `crs`.
    #[must_use]
    pub fn new(crs: Arc<Crs>) -> CrsMapping {
        CrsMapping { crs, latitude_range: OnceLock::new() }
    }

    /// The projection's coordinate reference system.
    #[must_use]
    pub fn crs(&self) -> &Arc<Crs> {
        &self.crs
    }

    /// Each lng/lat in degrees of `lng_lats` to its place in the world
    /// square, into `world`, which is as long: NaN where the projection has
    /// none.
    pub fn world_from_lng_lats(&self, lng_lats: &[[f64; 2]], world: &mut [[f64; 2]]) {
        if lng_lats.is_empty() {
            return;
        }
        self.crs.converter.forward(lng_lats, world);
        let TileMatrixSet { origin: [origin_x, origin_y], extent_at_zoom0: extent } = self.crs.tile_matrix_set;
        for position in world {
            *position = [(position[0] - origin_x) / extent, (origin_y - position[1]) / extent];
        }
    }

    /// Each place in the world square of `world` to lng/lat in degrees, into
    /// `lng_lats`, which is as long, with the latitude within 90 degrees of
    /// the equator. A place that has no answer of its own takes the nearest
    /// one inside the world square, or else the last one with an answer on
    /// the line to the square's center, or else the converter's own finite
    /// answer there; NaN where there is none of these.
    pub fn lng_lats_from_world(&self, world: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        self.own_answers(world, lng_lats);
        // The positions still without an answer, by index, and their nearest
        // places inside the world square.
        let mut rest: Vec<usize> = Vec::new();
        let mut inside: Vec<[f64; 2]> = Vec::new();
        for (index, (point, lng_lat)) in world.iter().zip(lng_lats.iter()).enumerate() {
            if !is_answer(lng_lat) {
                rest.push(index);
                inside.push([point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)]);
            }
        }
        fall_back(&mut rest, &mut inside, lng_lats, |inside, found| self.own_answers(inside, found));
        fall_back(&mut rest, &mut inside, lng_lats, |inside, found| self.answers_toward_center(inside, found));
        fall_back(&mut rest, &mut inside, lng_lats, |inside, found| {
            let positions: Vec<[f64; 2]> = inside.iter().map(|point| self.crs_position(*point)).collect();
            self.finite_inverses(&positions, found);
        });
        for lng_lat in lng_lats {
            lng_lat[1] = lng_lat[1].clamp(-90.0, 90.0);
        }
    }

    /// Whether the place `world` in the world square has a location of its
    /// own, rather than the nearest one that
    /// [`lng_lats_from_world`](Self::lng_lats_from_world) falls back on.
    #[must_use]
    pub fn has_lat_lng(&self, world: [f64; 2]) -> bool {
        let mut lng_lat = [[0.0; 2]];
        self.own_answers(&[world], &mut lng_lat);
        is_answer(&lng_lat[0]) && lng_lat[0][1].abs() <= 90.0
    }

    /// The latitudes, south and north, from the center of the tile matrix
    /// set toward each pole, up to which the projection stretches distances
    /// along the meridian less than ten times as much as at the center: the
    /// poles for a projection that stretches neither that much, as transverse
    /// Mercator does, and short of the pole that a polar projection puts at
    /// infinity. It is found once, when first asked for.
    #[must_use]
    pub fn latitude_range(&self) -> [f64; 2] {
        *self.latitude_range.get_or_init(|| {
            let mut center = [[0.0; 2]];
            self.lng_lats_from_world(&[[0.5, 0.5]], &mut center);
            [self.latitude_limit(center[0], -90.0), self.latitude_limit(center[0], 90.0)]
        })
    }

    /// The rectangle in the world square around a lng/lat box: the box
    /// sampled on a grid, 17 points on a side with its edges and corners,
    /// grown by how far its edges stray from the lines between neighboring
    /// samples. Corners alone miss most of a box whose edges curve, as a
    /// parallel does around a pole, and the inside of the box matters where
    /// a projection runs off to infinity.
    #[must_use]
    pub fn lng_lat_box_to_world_box(&self, bounds: &LatLngBounds) -> WorldBox {
        let BoxSamples { points, bulge } = self.lng_lat_box_samples(bounds);
        let mut min = Point::new(f64::INFINITY, f64::INFINITY);
        let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for point in &points {
            min = Point::new(min.x.min(point.x), min.y.min(point.y));
            max = Point::new(max.x.max(point.x), max.y.max(point.y));
        }
        WorldBox { min: Point::new(min.x - bulge, min.y - bulge), max: Point::new(max.x + bulge, max.y + bulge) }
    }

    /// The places in the world square of a lng/lat box sampled as
    /// [`lng_lat_box_to_world_box`](Self::lng_lat_box_to_world_box) samples
    /// it, and how far its edges stray from the lines between neighboring
    /// samples, which a shape that holds the samples grows by to hold the
    /// box.
    #[must_use]
    pub fn lng_lat_box_samples(&self, bounds: &LatLngBounds) -> BoxSamples {
        let longitude_at = |step: usize| {
            if step == BOX_SAMPLE_STEPS {
                bounds.east()
            } else {
                bounds.west() + (bounds.east() - bounds.west()) * step as f64 / BOX_SAMPLE_STEPS as f64
            }
        };
        let latitude_at = |step: usize| {
            if step == BOX_SAMPLE_STEPS {
                bounds.south()
            } else {
                bounds.north() + (bounds.south() - bounds.north()) * step as f64 / BOX_SAMPLE_STEPS as f64
            }
        };
        // The grid, row by row from the north-west, then the halfway points
        // along its edges: north, south, west and east for each step, all in
        // one batch.
        let mut lng_lats = Vec::with_capacity(BOX_SAMPLE_SIDE * BOX_SAMPLE_SIDE + 4 * BOX_SAMPLE_STEPS);
        for row in 0..BOX_SAMPLE_SIDE {
            lng_lats.extend((0..BOX_SAMPLE_SIDE).map(|column| [longitude_at(column), latitude_at(row)]));
        }
        for step in 0..BOX_SAMPLE_STEPS {
            let longitude_halfway = f64::midpoint(longitude_at(step), longitude_at(step + 1));
            let latitude_halfway = f64::midpoint(latitude_at(step), latitude_at(step + 1));
            lng_lats.extend([
                [longitude_halfway, bounds.north()],
                [longitude_halfway, bounds.south()],
                [bounds.west(), latitude_halfway],
                [bounds.east(), latitude_halfway],
            ]);
        }
        let mut world = vec![[0.0; 2]; lng_lats.len()];
        self.world_from_lng_lats(&lng_lats, &mut world);
        let (grid, halfway) = world.split_at(BOX_SAMPLE_SIDE * BOX_SAMPLE_SIDE);
        let at = |row: usize, column: usize| grid[row * BOX_SAMPLE_SIDE + column];

        let mut bulge = 0.0;
        for (step, &[north, south, west, east]) in halfway.as_chunks::<4>().0.iter().enumerate() {
            let edges = [
                (at(0, step), at(0, step + 1), north),
                (at(BOX_SAMPLE_STEPS, step), at(BOX_SAMPLE_STEPS, step + 1), south),
                (at(step, 0), at(step + 1, 0), west),
                (at(step, BOX_SAMPLE_STEPS), at(step + 1, BOX_SAMPLE_STEPS), east),
            ];
            for (start, end, middle) in edges {
                let chord = [end[0] - start[0], end[1] - start[1]];
                let to_middle = [middle[0] - start[0], middle[1] - start[1]];
                let chord_length = chord[0].hypot(chord[1]);
                let distance = if chord_length > 0.0 {
                    (chord[0] * to_middle[1] - chord[1] * to_middle[0]).abs() / chord_length
                } else {
                    to_middle[0].hypot(to_middle[1])
                };
                bulge = f64::max(bulge, distance);
            }
        }
        BoxSamples { points: grid.iter().map(|point| Point::new(point[0], point[1])).collect(), bulge }
    }

    /// The CRS position of a place in the world square.
    fn crs_position(&self, world: [f64; 2]) -> [f64; 2] {
        let TileMatrixSet { origin: [origin_x, origin_y], extent_at_zoom0: extent } = self.crs.tile_matrix_set;
        [origin_x + world[0] * extent, origin_y - world[1] * extent]
    }

    /// The converter's inverse of each CRS position, NaN where it isn't
    /// finite.
    fn finite_inverses(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        self.crs.converter.inverse(positions, lng_lats);
        for lng_lat in lng_lats {
            if !is_answer(lng_lat) {
                *lng_lat = NO_ANSWER;
            }
        }
    }

    /// The answer of each place in the world square of its own: the
    /// converter's inverse, where `forward` takes it back to the place's
    /// position within the tolerance; NaN elsewhere.
    fn own_answers(&self, world: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        if world.is_empty() {
            return;
        }
        let mut positions: Positions = world.iter().map(|point| self.crs_position(*point)).collect();
        self.finite_inverses(&positions, lng_lats);
        let Some(first) = lng_lats.iter().copied().find(is_answer) else {
            return;
        };
        // A position without an answer sends the first answer in its place,
        // so the converter sees only answers, and its own position becomes
        // NaN, so that the comparison fails.
        for (lng_lat, position) in lng_lats.iter_mut().zip(positions.iter_mut()) {
            if !is_answer(lng_lat) {
                *lng_lat = first;
                *position = NO_ANSWER;
            }
        }
        let mut back: Positions = smallvec![[0.0; 2]; world.len()];
        self.crs.converter.forward(lng_lats, &mut back);
        let tolerance = ROUND_TRIP_TOLERANCE * self.crs.tile_matrix_set.extent_at_zoom0;
        for ((lng_lat, forward), position) in lng_lats.iter_mut().zip(&back).zip(&positions) {
            let distance = (forward[0] - position[0]).hypot(forward[1] - position[1]);
            if distance.is_nan() || distance > tolerance {
                *lng_lat = NO_ANSWER;
            }
        }
    }

    /// Each place's answer at the edge of the answers on the line from it to
    /// the center of the world square, found by halving the line: it assumes
    /// the answers form one stretch from the center outwards. NaN where the
    /// center has no answer either.
    fn answers_toward_center(&self, world: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        let along_at =
            |point: &[f64; 2], fraction: f64| [0.5 + (point[0] - 0.5) * fraction, 0.5 + (point[1] - 0.5) * fraction];
        // For each place, the fraction of the way from the center that last
        // had an answer, and the one that last had none.
        let mut answered = vec![0.0; world.len()];
        let mut unanswered = vec![1.0; world.len()];
        let mut along = vec![[0.0; 2]; world.len()];
        for _ in 0..EDGE_SEARCH_STEPS {
            for (((along, point), answered), unanswered) in along.iter_mut().zip(world).zip(&answered).zip(&unanswered)
            {
                *along = along_at(point, f64::midpoint(*answered, *unanswered));
            }
            self.own_answers(&along, lng_lats);
            for ((lng_lat, answered), unanswered) in lng_lats.iter().zip(&mut answered).zip(&mut unanswered) {
                let fraction = f64::midpoint(*answered, *unanswered);
                if is_answer(lng_lat) {
                    *answered = fraction;
                } else {
                    *unanswered = fraction;
                }
            }
        }
        for ((along, point), answered) in along.iter_mut().zip(world).zip(&answered) {
            *along = along_at(point, *answered);
        }
        self.own_answers(&along, lng_lats);
    }

    /// The last latitude, from the center's toward `pole`, a whole degree at
    /// a time, before the projection stretches distances along the center's
    /// meridian more than [`MAX_MERIDIAN_STRETCH`] times as much as at the
    /// center; `pole` when it never does.
    fn latitude_limit(&self, center: [f64; 2], pole: f64) -> f64 {
        let [center_longitude, center_latitude] = center;
        let step = if pole > center_latitude { 1.0 } else { -1.0 };
        // Whole degrees from the center toward the pole, up to a degree short
        // of it.
        let steps = ((pole - center_latitude).abs() - 1.0).ceil().max(0.0) as usize;
        // How far a hundredth of a degree toward the equator moves a position
        // on the center's meridian: at the center, then a degree further
        // toward the pole from each whole degree, all in one batch.
        let toward_equator = |latitude: f64| if latitude >= 0.0 { latitude - 0.01 } else { latitude + 0.01 };
        let mut lng_lats = Vec::with_capacity(2 * (steps + 1));
        for latitude in
            std::iter::once(center_latitude).chain((0..steps).map(|index| center_latitude + index as f64 * step + step))
        {
            lng_lats.extend([[center_longitude, latitude], [center_longitude, toward_equator(latitude)]]);
        }
        let mut positions = vec![[0.0; 2]; lng_lats.len()];
        self.crs.converter.forward(&lng_lats, &mut positions);
        let stretches: Vec<f64> = positions
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[at, toward_equator]| (toward_equator[0] - at[0]).hypot(toward_equator[1] - at[1]))
            .collect();
        let Some((&at_center, beyond)) = stretches.split_first() else {
            return pole;
        };
        let max_stretch = MAX_MERIDIAN_STRETCH * at_center;
        if max_stretch.is_nan() || max_stretch <= 0.0 {
            return pole;
        }
        match beyond.iter().position(|stretch| stretch.is_nan() || *stretch > max_stretch) {
            Some(index) => center_latitude + index as f64 * step,
            None => pole,
        }
    }
}

/// Answers the positions of `lng_lats` that `rest` names, by index, with
/// `answer` of their places `inside` the world square, in one batch, and
/// leaves in `rest` and `inside` those still without an answer.
fn fall_back(
    rest: &mut Vec<usize>,
    inside: &mut Vec<[f64; 2]>,
    lng_lats: &mut [[f64; 2]],
    answer: impl FnOnce(&[[f64; 2]], &mut [[f64; 2]]),
) {
    if rest.is_empty() {
        return;
    }
    let mut found = vec![NO_ANSWER; rest.len()];
    answer(inside, &mut found);
    let (mut still, mut still_inside) = (Vec::new(), Vec::new());
    for ((&index, &point), &found) in rest.iter().zip(inside.iter()).zip(&found) {
        if is_answer(&found) {
            lng_lats[index] = found;
        } else {
            still.push(index);
            still_inside.push(point);
        }
    }
    (*rest, *inside) = (still, still_inside);
}
