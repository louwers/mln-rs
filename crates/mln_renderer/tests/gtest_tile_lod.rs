// Ports test/util/tile_lod.test.cpp (util::TileZoomFunction and
// util::elevationForTileCulling live in mln_renderer::util::tile_lod).

use mln_math::deg2rad;
use mln_renderer::util::tile_lod::{self, TileZoomFunction};

// TransformState's default field of view, in degrees.
const FOV: f64 = 36.869_897_645_844_02;

/// gtest's `EXPECT_DOUBLE_EQ`: equal within 4 ULPs.
fn assert_double_eq(actual: f64, expected: f64) {
    let ulps = |x: f64| -> i64 {
        let bits = x.to_bits() as i64;
        if bits < 0 { i64::MIN - bits } else { bits }
    };
    assert!(actual == expected || (ulps(actual) - ulps(expected)).abs() <= 4, "{actual} != {expected}");
}

/// gtest's `EXPECT_NEAR(val1, val2, abs_error)`.
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    assert!((val1 - val2).abs() <= abs_error, "|{val1} - {val2}| > {abs_error}");
}

/// `util::TileZoomFunction(fov)`: C++ defaults the other two arguments.
fn tile_zoom_function(camera_vertical_fov_degrees: f64) -> TileZoomFunction {
    TileZoomFunction::new(
        camera_vertical_fov_degrees,
        TileZoomFunction::DEFAULT_MAX_ZOOM_LEVELS_ON_SCREEN,
        TileZoomFunction::DEFAULT_TILE_COUNT_MAX_MIN_RATIO,
    )
}

// Oracle: GL JS `createCalculateTileZoomFunction(9.314, 3.0)` on the same inputs.
// gtest: TileLod.TileZoomMatchesGLJS
#[test]
fn tile_zoom_matches_gljs() {
    let tile_zoom = tile_zoom_function(FOV);
    // Straight down: the center tile loads at the requested zoom plus the field-of-view term.
    assert_near(tile_zoom.call(10.0, 0.0, 1.0, 1.0), 10.076_001_546_722_525, 1e-9);
    // A tile two camera heights away loads coarser.
    assert_near(tile_zoom.call(10.0, 2.0, 1.0, 1.0), 8.334_757_721_499_315, 1e-9);
    // Pitched 60 degrees: the center tile, a nearer one and a farther one.
    assert_near(tile_zoom.call(12.0, 3.0f64.sqrt() / 2.0, 0.5, 1.0), 11.576_175_751_895_164, 1e-9);
    assert_near(tile_zoom.call(12.0, 0.2, 0.5, 1.0), 12.915_426_593_532_23, 1e-9);
    assert_near(tile_zoom.call(12.0, 4.0, 0.5, 1.0), 8.559_750_250_763_466, 1e-9);
    // Toward the horizon.
    assert_near(tile_zoom.call(14.0, 20.0, deg2rad(80.0).cos(), 1.0), 6.331_340_340_282_265, 1e-9);
}

// Oracle: GL JS `createCalculateTileZoomFunction(1.0, 10.0)` and `(10.0, 1.0)`, the pairs its own tests use.
// gtest: TileLod.TileZoomParameters
#[test]
fn tile_zoom_parameters() {
    assert_near(TileZoomFunction::new(FOV, 1.0, 10.0).call(12.0, 4.0, 0.5, 1.0), 12.662_254_753_200_273, 1e-9);
    assert_near(TileZoomFunction::new(FOV, 10.0, 1.0).call(12.0, 4.0, 0.5, 1.0), 8.187_107_913_858_497, 1e-9);
}

// Oracle: GL JS `getElevationForTileCulling` on the same inputs.
// gtest: TileLod.ElevationForTileCulling
#[test]
fn elevation_for_tile_culling() {
    // C++ defaults centerElevation and maxContentElevation to 0.
    assert_double_eq(tile_lod::elevation_for_tile_culling(0.0, FOV, 0.0, 0.0), 0.0);
    assert_near(tile_lod::elevation_for_tile_culling(60.0, FOV, 0.0, 0.0), 139.498_294_097_400_33, 1e-9);
    assert_near(tile_lod::elevation_for_tile_culling(70.0, FOV, 0.0, 0.0), 472.831_627_430_733_65, 1e-9);
    assert_double_eq(tile_lod::elevation_for_tile_culling(85.0, FOV, 0.0, 0.0), 500.0);
    // Terrain lifts the allowance by the center elevation; taller content raises the assumed feature height.
    assert_near(tile_lod::elevation_for_tile_culling(70.0, FOV, 1200.0, 800.0), 1_956.530_603_889_173_8, 1e-9);
    assert_near(tile_lod::elevation_for_tile_culling(70.0, FOV, 0.0, 900.0), 851.096_929_375_320_5, 1e-9);
}
