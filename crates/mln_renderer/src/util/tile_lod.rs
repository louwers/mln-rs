// The C++ `TileZoomFunction::operator()` is `TileZoomFunction::call`.

use mln_math::deg2rad;

/// The pitch at which Mercator's horizon reaches the top of the screen, in
/// degrees.
pub const MAX_MERCATOR_HORIZON_ANGLE_DEGREES: f64 = 89.25;

const ASSUMED_MAX_FEATURE_HEIGHT_METERS: f64 = 500.0;
const TILE_CULLING_HORIZON_ONSET_DEGREES: f64 = 15.0;

fn integral_of_cos_x_by_p(p: f64, x1: f64, x2: f64) -> f64 {
    const NUM_POINTS: i32 = 10;
    let mut sum = 0.0;
    let dx = (x2 - x1) / f64::from(NUM_POINTS);
    for i in 0..NUM_POINTS {
        let x = x1 + (f64::from(i) + 0.5) / f64::from(NUM_POINTS) * (x2 - x1);
        sum += dx * x.cos().powf(p);
    }
    sum
}

/// The zoom each tile is loaded at when the view is pitched: finer for tiles
/// nearer the camera, coarser toward the horizon, under a total tile budget.
/// GL JS `createCalculateTileZoomFunction`, which GL JS applies to both
/// projections (the Mercator cover uses it whenever terrain is present or the
/// pitch passes 78.5 - fov/2 degrees).
#[derive(Clone, Copy, Debug)]
pub struct TileZoomFunction {
    camera_vertical_fov: f64,
    pitch_tile_loading_behavior: f64,
    tile_count_pitch0: f64,
    tile_count_max_min_ratio: f64,
}

impl TileZoomFunction {
    pub const DEFAULT_MAX_ZOOM_LEVELS_ON_SCREEN: f64 = 9.314;
    pub const DEFAULT_TILE_COUNT_MAX_MIN_RATIO: f64 = 3.0;

    /// The terms that depend on the field of view alone are computed once
    /// here, for a whole cover. (C++ defaults the last two arguments to
    /// `DEFAULT_MAX_ZOOM_LEVELS_ON_SCREEN` and
    /// `DEFAULT_TILE_COUNT_MAX_MIN_RATIO`.)
    #[must_use]
    pub fn new(
        camera_vertical_fov_degrees: f64,
        max_zoom_levels_on_screen: f64,
        tile_count_max_min_ratio: f64,
    ) -> Self {
        let pitch_tile_loading_behavior = 2.0
            * ((max_zoom_levels_on_screen - 1.0)
                / (deg2rad(MAX_MERCATOR_HORIZON_ANGLE_DEGREES - camera_vertical_fov_degrees).cos()
                    / deg2rad(MAX_MERCATOR_HORIZON_ANGLE_DEGREES).cos())
                .log2()
                - 1.0);
        TileZoomFunction {
            camera_vertical_fov: camera_vertical_fov_degrees,
            pitch_tile_loading_behavior,
            tile_count_pitch0: 2.0
                * integral_of_cos_x_by_p(
                    pitch_tile_loading_behavior - 1.0,
                    0.0,
                    deg2rad(camera_vertical_fov_degrees / 2.0),
                ),
            tile_count_max_min_ratio,
        }
    }

    /// The zoom to load a tile at. Distances share one unit (GL JS uses
    /// Mercator units at zoom 0). Returns an unrounded zoom.
    #[must_use]
    pub fn call(
        &self,
        requested_center_zoom: f64,
        distance_to_tile_2d: f64,
        distance_to_tile_z: f64,
        distance_to_center_3d: f64,
    ) -> f64 {
        let center_pitch = f64::min(1.0, distance_to_tile_z / distance_to_center_3d).acos();
        let highest_pitch = f64::min(
            deg2rad(MAX_MERCATOR_HORIZON_ANGLE_DEGREES),
            center_pitch + deg2rad(self.camera_vertical_fov / 2.0),
        );
        let lowest_pitch = f64::min(highest_pitch, center_pitch - deg2rad(self.camera_vertical_fov / 2.0));
        let tile_count = integral_of_cos_x_by_p(self.pitch_tile_loading_behavior - 1.0, lowest_pitch, highest_pitch);
        let this_tile_pitch = (distance_to_tile_2d / distance_to_tile_z).atan();
        let distance_to_tile_3d = distance_to_tile_2d.hypot(distance_to_tile_z);
        let mut desired = requested_center_zoom;
        desired += (distance_to_center_3d
            / distance_to_tile_3d
            / f64::max(0.5, deg2rad(self.camera_vertical_fov / 2.0).cos()))
        .log2();
        desired += self.pitch_tile_loading_behavior * this_tile_pitch.cos().log2() / 2.0;
        desired -= f64::max(1.0, tile_count / self.tile_count_pitch0 / self.tile_count_max_min_ratio).log2() / 2.0;
        desired
    }
}

/// Near the bottom of a steeply pitched view, tiles are culled with room for
/// the tallest feature they might hold, so buildings do not vanish before
/// their tile leaves the screen. GL JS `getElevationForTileCulling`:
/// `center_elevation` is the terrain height under the map center and
/// `max_content_elevation` the tallest content the style is known to hold;
/// both are zero without terrain (C++ defaults them to 0).
#[must_use]
pub fn elevation_for_tile_culling(
    pitch_degrees: f64,
    fov_degrees: f64,
    center_elevation: f64,
    max_content_elevation: f64,
) -> f64 {
    let bottom_edge_degrees_above_horizontal = MAX_MERCATOR_HORIZON_ANGLE_DEGREES - pitch_degrees - fov_degrees / 2.0;
    let proximity = (TILE_CULLING_HORIZON_ONSET_DEGREES - bottom_edge_degrees_above_horizontal)
        / TILE_CULLING_HORIZON_ONSET_DEGREES;
    let proximity_to_horizon = proximity.clamp(0.0, 1.0);
    center_elevation + proximity_to_horizon * f64::max(ASSUMED_MAX_FEATURE_HEIGHT_METERS, max_content_elevation)
}
