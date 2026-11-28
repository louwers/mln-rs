//! The worked examples of Ordnance Survey's "A Guide to Coordinate Systems in
//! Great Britain", V3.6 (2020), and the registered projection's round trips:
//! MapLibre Native's `test/util/national_grid.test.cpp`.

use mln_engine::geo::constants::EARTH_RADIUS_M;

use super::*;

fn degrees(whole_degrees: f64, minutes: f64, seconds: f64) -> f64 {
    whole_degrees + minutes / 60.0 + seconds / 3600.0
}

const MILLIMETER: f64 = 1e-3;

/// The guide gives angles to 0.0001 arc seconds, so they are good to half of
/// that.
const GUIDE_ANGLE_TOLERANCE: f64 = 0.00005 / 3600.0;

#[track_caller]
fn assert_near(expected: f64, actual: f64, tolerance: f64) {
    assert!((expected - actual).abs() <= tolerance, "{actual} is not within {tolerance} of {expected}");
}

// gtest: NationalGrid.ProjectsTheGuidesTransverseMercatorExample
#[test]
fn projects_the_guides_transverse_mercator_example() {
    let [easting, northing] =
        NATIONAL_GRID_PROJECTION.forward(degrees(52.0, 39.0, 27.2531), degrees(1.0, 43.0, 4.5177));
    assert_near(651_409.903, easting, MILLIMETER);
    assert_near(313_177.270, northing, MILLIMETER);
}

// gtest: NationalGrid.UnprojectsTheGuidesTransverseMercatorExample
#[test]
fn unprojects_the_guides_transverse_mercator_example() {
    let [latitude, longitude] = NATIONAL_GRID_PROJECTION.inverse(651_409.903, 313_177.270);
    assert_near(degrees(52.0, 39.0, 27.2531), latitude, GUIDE_ANGLE_TOLERANCE);
    assert_near(degrees(1.0, 43.0, 4.5177), longitude, GUIDE_ANGLE_TOLERANCE);

    let [easting, northing] = NATIONAL_GRID_PROJECTION.forward(latitude, longitude);
    assert_near(651_409.903, easting, MILLIMETER);
    assert_near(313_177.270, northing, MILLIMETER);
}

// gtest: NationalGrid.TransformsTheGuidesHelmertExample
#[test]
fn transforms_the_guides_helmert_example() {
    let position = LatLngHeight {
        latitude: degrees(53.0, 36.0, 43.1653),
        longitude: -degrees(1.0, 39.0, 51.9920),
        height: 299.800,
    };
    let etrs89 = cartesian_from_lat_lng_height(GRS80, position);
    assert_near(3_790_644.900, etrs89[0], MILLIMETER);
    assert_near(-110_149.210, etrs89[1], MILLIMETER);
    assert_near(5_111_482.970, etrs89[2], MILLIMETER);

    let osgb36 = ETRS89_TO_OSGB36.apply(etrs89);
    assert_near(3_790_269.549, osgb36[0], MILLIMETER);
    assert_near(-110_038.064, osgb36[1], MILLIMETER);
    assert_near(5_111_050.261, osgb36[2], MILLIMETER);

    let on_airy = lat_lng_height_from_cartesian(AIRY_1830, osgb36);
    assert_near(degrees(53.0, 36.0, 42.2972), on_airy.latitude, GUIDE_ANGLE_TOLERANCE);
    assert_near(-degrees(1.0, 39.0, 46.5416), on_airy.longitude, GUIDE_ANGLE_TOLERANCE);
    assert_near(249.950, on_airy.height, MILLIMETER);

    let [easting, northing] = NATIONAL_GRID_PROJECTION.forward(on_airy.latitude, on_airy.longitude);
    assert_near(422_297.792, easting, MILLIMETER);
    assert_near(412_878.741, northing, MILLIMETER);
}

// gtest: NationalGrid.RegisteredProjectionConvertsTheHelmertExampleAtSeaLevel
// The example's point is 299.8 m up, and the registered projection converts
// at sea level, which moves the point 6 mm.
#[test]
fn the_registered_projection_converts_the_helmert_example_at_sea_level() {
    let centimeter = 1e-2;
    let crs = national_grid("EPSG:27700");
    let lng_lat = [[-degrees(1.0, 39.0, 51.9920), degrees(53.0, 36.0, 43.1653)]];
    let mut position = [[0.0; 2]];
    crs.converter.forward(&lng_lat, &mut position);
    assert_near(422_297.792, position[0][0], centimeter);
    assert_near(412_878.741, position[0][1], centimeter);

    let mut back = [[0.0; 2]];
    crs.converter.inverse(&position, &mut back);
    let mut again = [[0.0; 2]];
    crs.converter.forward(&back, &mut again);
    assert_near(position[0][0], again[0][0], centimeter);
    assert_near(position[0][1], again[0][1], centimeter);
}

// gtest: NationalGrid.RegisteredProjectionsInverseUndoesItsForward
// A camera move takes its anchor to lat/lng and back on every step, so any
// gap between the two adds up over a pinch: both round trips meet within a
// micrometer over Great Britain on the grid, every 50 km.
#[test]
fn the_registered_projections_inverse_undoes_its_forward() {
    let micrometer = 1e-6;
    let meters_per_degree = EARTH_RADIUS_M * RADIANS_PER_DEGREE;
    let crs = national_grid("EPSG:27700");
    let (max_easting, max_northing, step) = (700_000.0, 1_250_000.0, 50_000.0);
    let mut positions = Vec::new();
    for column in (0..).take_while(|column| f64::from(*column) * step <= max_easting) {
        for row in (0..).take_while(|row| f64::from(*row) * step <= max_northing) {
            positions.push([f64::from(column) * step, f64::from(row) * step]);
        }
    }
    let mut lng_lats = vec![[0.0; 2]; positions.len()];
    let mut again = vec![[0.0; 2]; positions.len()];
    let mut lng_lats_again = vec![[0.0; 2]; positions.len()];
    crs.converter.inverse(&positions, &mut lng_lats);
    crs.converter.forward(&lng_lats, &mut again);
    crs.converter.inverse(&again, &mut lng_lats_again);

    for (((position, lng_lat), again), lng_lat_again) in
        positions.iter().zip(&lng_lats).zip(&again).zip(&lng_lats_again)
    {
        let at = format!("{} E {} N", position[0], position[1]);
        assert!((again[0] - position[0]).hypot(again[1] - position[1]) < micrometer, "{at}");
        let latitude = lng_lat[1] * RADIANS_PER_DEGREE;
        let east = (lng_lat_again[0] - lng_lat[0]) * latitude.cos();
        let north = lng_lat_again[1] - lng_lat[1];
        assert!(east.hypot(north) * meters_per_degree < micrometer, "{at}");
    }
}
