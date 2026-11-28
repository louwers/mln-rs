//! Where a line of sight meets the ground a frame drew.

use geometry::Point;
use glam::{DVec2, DVec3};
use mln_style::constants::{LATITUDE_MAX, TILE_SIZE_D};
use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};

use crate::camera::vertical_perspective_projection::VerticalPerspectiveProjection;
use crate::camera::{Elevation, View};
use crate::geo::ScreenCoordinate;
use crate::geo::projection::Projection;
use crate::geo::world::WorldMapping;

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
pub(crate) struct GroundHit {
    /// Its point in the map's world square at zoom 0, Web Mercator's or a
    /// registered projection's (from 0 to 1 across, y south), with x in the
    /// world copy the view shows it in. On a globe's polar cap y is past 0
    /// or 1.
    pub(crate) world: DVec2,
    /// The height of the surface there, in meters with the terrain's
    /// exaggeration ([`Elevation::surface_at`]).
    pub(crate) height: f64,
}

impl GroundHit {
    /// Its location where lng/lat maps to the world square as `mapping`,
    /// the map's, maps it: in the world copy of [`world`](Self::world)
    /// unless `wrap_mode` wraps it, which a registered projection's world
    /// doesn't.
    #[must_use]
    pub(crate) fn location(&self, mapping: &WorldMapping, wrap_mode: WrapMode) -> LatLng {
        let world = Point::new(self.world.x, self.world.y);
        // A finite point of the world always has a location.
        mapping.unproject(&world, 1.0 / TILE_SIZE_D, wrap_mode).unwrap_or_default()
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
    /// that ends under the ground. On the globe the band lies between the
    /// spheres of the lowest and highest heights and sea level, a point's
    /// height is measured up its vertical from the sea-level sphere, and the
    /// hit's [`world`](GroundHit::world) is in the world copy nearest the
    /// view's center; past the Mercator range, where the globe closes its
    /// poles at sea level, the ground is at sea level wherever the tile at
    /// the range's edge below it drew. `None` where the line meets no drawn
    /// ground, and while the view shows nothing.
    #[must_use]
    pub(crate) fn ground_under(&self, point: &ScreenCoordinate, ground: &dyn Elevation) -> Option<GroundHit> {
        if self.is_globe() {
            return self.ground_under_on_globe(point, ground);
        }
        let (near, far) = self.line_of_sight(point)?;
        self.ground_along(near, far, ground)
    }

    /// The location that `point`, measured from the bottom edge (y up),
    /// shows over terrain: where its line of sight first meets `ground`
    /// ([`ground_under`](Self::ground_under)). Where it meets none, as in
    /// the sky or past the ground drawn, the plane at the center's altitude
    /// stands in for the ground, on the globe the sphere
    /// ([`screen_coordinate_to_lat_lng_at`](Self::screen_coordinate_to_lat_lng_at)).
    /// Fails as [`screen_coordinate_to_lat_lng`](Self::screen_coordinate_to_lat_lng).
    pub(crate) fn screen_coordinate_to_lat_lng_on(
        &self,
        point: &ScreenCoordinate,
        ground: &dyn Elevation,
        wrap_mode: WrapMode,
    ) -> Result<LatLng, Error> {
        match self.ground_under(point, ground) {
            Some(hit) => Ok(hit.location(self.world(), wrap_mode)),
            None => self.screen_coordinate_to_lat_lng_at(point, self.center_altitude(), wrap_mode),
        }
    }

    /// The screen point (y up) of `lat_lng` standing on `ground`: at the
    /// height the ground was drawn at there, read in the world copy nearest
    /// the center, or at the center's altitude where none was drawn, the
    /// height [`screen_coordinate_to_lat_lng_on`] falls back to. On the
    /// globe, past the Mercator range, the ground stands at sea level, as
    /// [`ground_under`](Self::ground_under) meets it.
    ///
    /// [`screen_coordinate_to_lat_lng_on`]: Self::screen_coordinate_to_lat_lng_on
    #[must_use]
    pub(crate) fn lat_lng_to_screen_coordinate_on(&self, lat_lng: &LatLng, ground: &dyn Elevation) -> ScreenCoordinate {
        let height = self.drawn_height_at(lat_lng, ground);
        self.lat_lng_to_screen_coordinate_at(lat_lng, height.unwrap_or_else(|| self.center_altitude()))
    }

    /// The height `ground` was drawn at, at `lat_lng`, read in the world
    /// copy nearest the center: on the globe, past the Mercator range, the
    /// sea-level polar caps. `None` where no ground was drawn there, though
    /// the camera may stand on ground under its center that wasn't drawn
    /// ([`Elevation::at`]).
    #[must_use]
    pub(crate) fn drawn_height_at(&self, lat_lng: &LatLng, ground: &dyn Elevation) -> Option<f64> {
        let near = self.lat_lng_near_center(lat_lng);
        if self.is_globe() && near.latitude().abs() > LATITUDE_MAX {
            return Some(0.0);
        }
        let world = self.world().project(&near, 1.0 / TILE_SIZE_D);
        ground.surface_at(DVec2::new(world.x, world.y))
    }

    /// [`ground_under`](Self::ground_under) on the globe, in unit-sphere
    /// space.
    fn ground_under_on_globe(&self, point: &ScreenCoordinate, ground: &dyn Elevation) -> Option<GroundHit> {
        let size = self.size();
        if size.is_empty() {
            return None;
        }
        // The band holds sea level, where the globe closes its poles.
        let (low, high) = ground.range()?;
        let radius = |height: f64| 1.0 + height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS;
        let (low, high) = (radius(low.min(0.0) - BAND_PADDING), radius(high.max(0.0) + BAND_PADDING));

        // The line of sight from the near plane to the far plane.
        let inverse = self.inverse_globe_view_projection_matrix();
        let ndc = DVec2::new(point.x / f64::from(size.width) * 2.0 - 1.0, point.y / f64::from(size.height) * 2.0 - 1.0);
        let near = inverse.project_point3(ndc.extend(-1.0));
        let along = inverse.project_point3(ndc.extend(1.0)) - near;
        if !(near.is_finite() && along.is_finite()) {
            return None;
        }

        // The stretch of it within the shell of the ground's heights: from
        // where it enters the outer sphere, or the near plane inside it, to
        // where it enters the inner sphere, or else leaves the outer one.
        let (outer_in, outer_out) = sphere_crossings(near, along, high)?;
        let start = outer_in.max(0.0);
        let end = sphere_crossings(near, along, low).map_or(outer_out, |(inner_in, _)| inner_in).min(1.0);
        if start >= end {
            return None;
        }

        let point_at = |t: f64| near + along * t;
        let sample = |t: f64| {
            let world = VerticalPerspectiveProjection::sphere_point_to_world(self, point_at(t))?;
            // A polar cap draws at sea level where the tile at the edge of
            // Web Mercator's square below it drew.
            let edge = DVec2::new(world.x, world.y.clamp(0.0, 1.0));
            let surface = ground.surface_at(edge)?;
            Some((world, if edge.y == world.y { surface } else { 0.0 }))
        };
        // The ground the band spans, from the directions of its ends.
        let globe_radius = VerticalPerspectiveProjection::globe_radius_pixels(
            Projection::world_size(self.scale()),
            self.center(WrapMode::Unwrapped).latitude(),
        );
        let band_px = (point_at(start).normalize() - point_at(end).normalize()).length() * globe_radius;
        march(start, end, band_px / STEP_PX, BISECTION_PX * (end - start) / band_px, sample)
    }

    /// Where the line from `near` to `far`, each a Web Mercator point at
    /// zoom 0 (x in the world copy the view shows it in) and a height in
    /// meters, first enters `ground` from above, as
    /// [`ground_under`](Self::ground_under) finds it on a line of sight,
    /// sampling every 4 pixels across the ground at the view's zoom. `None`
    /// where the line meets no drawn ground.
    #[must_use]
    pub(crate) fn ground_along(&self, near: DVec3, far: DVec3, ground: &dyn Elevation) -> Option<GroundHit> {
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
        let sample = |t: f64| {
            let point = point_at(t);
            Some((point, ground.surface_at(point.truncate())?))
        };
        let length_px = along.truncate().length() * Projection::world_size(self.scale());
        march(start, end, length_px * (end - start) / STEP_PX, BISECTION_PX / length_px, sample)
    }
}

/// Where a line, from `start` to `end` of its parameter, first enters the
/// ground from above, or next enters it where it starts under it: `sample`
/// gives the line's Web Mercator point at zoom 0 and height in meters at a
/// parameter, with the surface's height there, or `None` where no ground
/// was drawn. It samples the line `steps` times, at most 512, spaced closer
/// toward `start` where it would take more, and bisects the first step that
/// ends under the ground to within `tolerance` of the parameter.
fn march(
    start: f64,
    end: f64,
    steps: f64,
    tolerance: f64,
    sample: impl Fn(f64) -> Option<(DVec3, f64)>,
) -> Option<GroundHit> {
    let surface = |t: f64| sample(t).map(|(_, surface)| surface);
    let below = |t: f64| sample(t).is_some_and(|(point, surface)| point.z <= surface + HIT_EPSILON);

    let steps = steps.ceil();
    let samples = steps.clamp(1.0, f64::from(MAX_SAMPLES)) as u32;
    let capped = steps > f64::from(MAX_SAMPLES);
    let at = |i: u32| {
        let f = f64::from(i) / f64::from(samples);
        start + (end - start) * if capped { f * f } else { f }
    };

    let mut above = !below(start);
    let mut previous = start;
    for i in 1..=samples {
        let t = at(i);
        if !above {
            above = !below(t);
        } else if below(t) {
            let (mut lo, mut hi) = (previous, t);
            // A line that runs straight down needs no bisection: its surface
            // doesn't change along it.
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
            let height_at = |t: f64| sample(t).map_or(f64::NAN, |(point, _)| point.z);
            let (t, height) =
                refine(lo, hi, height_at, surface).unwrap_or_else(|| (hi, surface(hi).unwrap_or_default()));
            let point = sample(t).map_or(DVec3::NAN, |(point, _)| point);
            return Some(GroundHit { world: point.truncate(), height });
        }
        previous = t;
    }
    None
}

/// Where a line from `origin` along `along` crosses the sphere of `radius`
/// about the planet's center, in units of `along`: where it goes in and
/// where it comes out, `None` where it misses.
fn sphere_crossings(origin: DVec3, along: DVec3, radius: f64) -> Option<(f64, f64)> {
    let a = along.length_squared();
    let b = origin.dot(along);
    // The line's nearest point to the center, then the half chord either side
    // of it, which stays exact where |origin| is near `radius`.
    let nearest = -b / a;
    let closest = origin + along * nearest;
    let half_chord_sq = (radius * radius - closest.length_squared()) / a;
    if half_chord_sq.is_nan() || half_chord_sq < 0.0 {
        return None;
    }
    let half_chord = half_chord_sq.sqrt();
    Some((nearest - half_chord, nearest + half_chord))
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
