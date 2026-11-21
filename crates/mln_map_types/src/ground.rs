//! Where a line of sight meets the ground a frame drew.

use geometry::Point;
use glam::{DVec2, DVec3};
use mln_util::constants::TILE_SIZE_D;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::projection::Projection;

use crate::{Elevation, View};

/// How far apart, in pixels at the zoom across the ground, a ray samples the
/// ground, and the most samples it takes (GL JS's `raycastTerrain`).
const STEP_PX: f64 = 4.0;
const MAX_SAMPLES: u32 = 512;

/// How near, in pixels at the zoom across the ground, bisection brings a
/// hit, in at most [`MAX_BISECTIONS`] steps.
const BISECTION_PX: f64 = 1e-3;
const MAX_BISECTIONS: u32 = 40;

/// How far above the surface a point still counts as on it, in meters.
const HIT_EPSILON: f64 = 1e-6;

/// How far, in meters, a ray looks past the snapshot's lowest and highest
/// ground, which keeps the band open over flat ground.
const BAND_PADDING: f64 = 10.0;

/// A point of the ground a frame drew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundHit {
    /// Its Web Mercator point at zoom 0 (from 0 to 1 across, y south), with
    /// x in the world copy the view shows it in.
    pub world: DVec2,
    /// The height of the surface there, in meters with the terrain's
    /// exaggeration ([`Elevation::surface_at`]).
    pub height: f64,
}

impl GroundHit {
    /// Its location, in the world copy of [`world`](Self::world) unless
    /// `wrap_mode` wraps it.
    #[must_use]
    pub fn location(&self, wrap_mode: WrapMode) -> LatLng {
        let world = Point::new(self.world.x, self.world.y);
        // A finite point of the world always has a location.
        Projection::unproject(&world, 1.0 / TILE_SIZE_D, wrap_mode).unwrap_or_default()
    }
}

impl View {
    /// Where the line of sight through `point`, measured from the bottom
    /// edge (y up), first enters `ground` from above, between the near and
    /// far planes; a line that starts under the ground finds where it next
    /// enters it (GL JS's `raycastTerrain`). It samples the surface the
    /// frame drew ([`Elevation::surface_at`]) every 4 pixels across the
    /// ground within the band of its heights, at most 512 times, which a
    /// long line spaces closer near the eye, and bisects the first step
    /// that ends under the ground. `None` where the line meets no drawn
    /// ground, on the globe, and while the view shows nothing.
    #[must_use]
    pub fn ground_under(&self, point: &ScreenCoordinate, ground: &dyn Elevation) -> Option<GroundHit> {
        let (near, far) = self.line_of_sight(point)?;
        self.ground_along(near, far, ground)
    }

    /// Where the line from `near` to `far`, each a Web Mercator point at
    /// zoom 0 (x in the world copy the view shows it in) and a height in
    /// meters, first enters `ground` from above, as
    /// [`ground_under`](Self::ground_under) finds it on a line of sight,
    /// sampling every 4 pixels across the ground at the view's zoom. `None`
    /// where the line meets no drawn ground.
    #[must_use]
    pub fn ground_along(&self, near: DVec3, far: DVec3, ground: &dyn Elevation) -> Option<GroundHit> {
        let (low, high) = ground.range()?;
        let (low, high) = (low - BAND_PADDING, high + BAND_PADDING);
        let along = far - near;

        // The stretch of the line within the band of the ground's heights.
        let (start, end) = if along.z == 0.0 {
            if near.z < low || near.z > high {
                return None;
            }
            (0.0, 1.0)
        } else {
            let (to_high, to_low) = ((high - near.z) / along.z, (low - near.z) / along.z);
            let (start, end) = (to_high.min(to_low).max(0.0), to_high.max(to_low).min(1.0));
            if start > end {
                return None;
            }
            (start, end)
        };

        let point_at = |t: f64| near + along * t;
        let surface = |t: f64| ground.surface_at(point_at(t).truncate());
        let below = |t: f64| surface(t).is_some_and(|surface| point_at(t).z <= surface + HIT_EPSILON);

        let length_px = along.truncate().length() * Projection::world_size(self.scale());
        let steps = (length_px * (end - start) / STEP_PX).ceil();
        let samples = steps.clamp(1.0, f64::from(MAX_SAMPLES)) as u32;
        let capped = steps > f64::from(MAX_SAMPLES);
        let sample = |i: u32| {
            let f = f64::from(i) / f64::from(samples);
            start + (end - start) * if capped { f * f } else { f }
        };

        let mut above = !below(start);
        let mut previous = start;
        for i in 1..=samples {
            let t = sample(i);
            if !above {
                above = !below(t);
            } else if below(t) {
                let (mut lo, mut hi) = (previous, t);
                // A line that runs straight down needs no bisection: its
                // surface doesn't change along it.
                let tolerance = BISECTION_PX / length_px;
                for _ in 0..MAX_BISECTIONS {
                    if hi - lo <= tolerance {
                        break;
                    }
                    let mid = f64::midpoint(lo, hi);
                    if below(mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                // `hi` is under drawn ground.
                let (t, height) =
                    refine(lo, hi, |t| point_at(t).z, surface).unwrap_or_else(|| (hi, surface(hi).unwrap_or_default()));
                return Some(GroundHit { world: point_at(t).truncate(), height });
            }
            previous = t;
        }
        None
    }
}

/// Where between `lo`, above the ground, and `hi`, under it, a line of
/// heights `z` crosses the `surface`, taking both as straight between them,
/// and the surface's height there; `None` where the surface isn't drawn at
/// `lo` or where it would be.
fn refine(lo: f64, hi: f64, z: impl Fn(f64) -> f64, surface: impl Fn(f64) -> Option<f64>) -> Option<(f64, f64)> {
    let over_lo = z(lo) - surface(lo)?;
    let over_hi = z(hi) - surface(hi)?;
    let t = if over_lo > over_hi { (lo + over_lo * (hi - lo) / (over_lo - over_hi)).clamp(lo, hi) } else { hi };
    Some((t, surface(t)?))
}

#[cfg(test)]
#[path = "ground_test.rs"]
mod ground_test;
