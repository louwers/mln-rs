// Ports test/util/projection.test.cpp

use super::*;

// TODO(port): use mln_util::constants::DEFAULT_MAX_ZOOM once it exists.
const DEFAULT_MAX_ZOOM: u8 = 22;

/// gtest `EXPECT_DOUBLE_EQ`: within 4 ULPs, NaN never equal.
fn double_eq(a: f64, b: f64) -> bool {
    fn biased(f: f64) -> u64 {
        let bits = f.to_bits();
        if bits & 0x8000_0000_0000_0000 != 0 { (!bits).wrapping_add(1) } else { 0x8000_0000_0000_0000 | bits }
    }
    if a.is_nan() || b.is_nan() {
        return false;
    }
    let (ba, bb) = (biased(a), biased(b));
    ba.abs_diff(bb) <= 4
}

macro_rules! expect_double_eq {
    ($a:expr, $b:expr) => {{
        let (a, b): (f64, f64) = ($a, $b);
        assert!(double_eq(a, b), "EXPECT_DOUBLE_EQ({}, {}): {:?} vs {:?}", stringify!($a), stringify!($b), a, b);
    }};
}

macro_rules! expect_near {
    ($a:expr, $b:expr, $tol:expr) => {{
        let (a, b, tol): (f64, f64, f64) = ($a, $b, $tol);
        assert!(
            (a - b).abs() <= tol,
            "EXPECT_NEAR({}, {}, {}): {:?} vs {:?}",
            stringify!($a),
            stringify!($b),
            stringify!($tol),
            a,
            b
        );
    }};
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

// gtest: Projection.Boundaries
#[test]
fn boundaries() {
    let sw = ll(-90.0, -180.0);
    let ne = ll(90.0, 180.0);

    let min_scale = 2.0f64.powf(0.0);
    let max_scale = 2.0f64.powf(f64::from(DEFAULT_MAX_ZOOM));

    let mut projected = Projection::project(&sw, min_scale);
    expect_double_eq!(projected.x, 0.0);
    expect_double_eq!(projected.y, TILE_SIZE_D);

    let mut unprojected = Projection::unproject(&projected, min_scale, WrapMode::Unwrapped).unwrap();
    expect_double_eq!(unprojected.latitude(), -LATITUDE_MAX);
    expect_double_eq!(unprojected.longitude(), sw.longitude());

    projected = Projection::project(&sw, max_scale);
    expect_double_eq!(projected.x, 0.0);
    expect_double_eq!(projected.y, TILE_SIZE_D * max_scale);

    unprojected = Projection::unproject(&projected, max_scale, WrapMode::Unwrapped).unwrap();
    expect_double_eq!(unprojected.latitude(), -LATITUDE_MAX);
    expect_double_eq!(unprojected.longitude(), sw.longitude());

    projected = Projection::project(&ne, min_scale);
    expect_double_eq!(projected.x, TILE_SIZE_D);
    expect_near!(projected.y, 0.0, 1e-10);

    unprojected = Projection::unproject(&projected, min_scale, WrapMode::Unwrapped).unwrap();
    expect_double_eq!(unprojected.latitude(), LATITUDE_MAX);
    expect_double_eq!(unprojected.longitude(), ne.longitude());

    projected = Projection::project(&ne, max_scale);
    expect_double_eq!(projected.x, TILE_SIZE_D * max_scale);
    expect_near!(projected.y, 0.0, 1e-6);

    unprojected = Projection::unproject(&projected, max_scale, WrapMode::Unwrapped).unwrap();
    expect_double_eq!(unprojected.latitude(), LATITUDE_MAX);
    expect_double_eq!(unprojected.longitude(), ne.longitude());
}

// gtest: Projection.MetersPerPixelAtLatitude
#[test]
fn meters_per_pixel_at_latitude() {
    let mut zoom = 0.0;
    expect_near!(Projection::get_meters_per_pixel_at_latitude(0.0, zoom), 78_271.516_964_020_48, 1e-7);
    expect_near!(Projection::get_meters_per_pixel_at_latitude(-LATITUDE_MAX, zoom), 6_752.228_472_944_65, 1e-7);
    expect_double_eq!(
        Projection::get_meters_per_pixel_at_latitude(LATITUDE_MAX, zoom),
        Projection::get_meters_per_pixel_at_latitude(-LATITUDE_MAX, zoom)
    );

    zoom = 20.0;
    expect_near!(Projection::get_meters_per_pixel_at_latitude(0.0, zoom), 0.074_645_535_434_742_44, 1e-7);
    expect_near!(Projection::get_meters_per_pixel_at_latitude(-LATITUDE_MAX, zoom), 0.006_439_426_873_154_306_5, 1e-7);

    // std::numeric_limits<double>::lowest() / max()
    expect_double_eq!(
        Projection::get_meters_per_pixel_at_latitude(f64::MIN, zoom),
        Projection::get_meters_per_pixel_at_latitude(f64::MAX, zoom)
    );

    // std::numeric_limits<double>::min() is the smallest positive normal.
    zoom = f64::MIN_POSITIVE;
    expect_double_eq!(
        Projection::get_meters_per_pixel_at_latitude(0.0, zoom),
        Projection::get_meters_per_pixel_at_latitude(0.0, MIN_ZOOM)
    );

    zoom = f64::MIN;
    expect_double_eq!(
        Projection::get_meters_per_pixel_at_latitude(0.0, zoom),
        Projection::get_meters_per_pixel_at_latitude(0.0, MIN_ZOOM)
    );

    zoom = f64::MAX;
    expect_double_eq!(
        Projection::get_meters_per_pixel_at_latitude(0.0, zoom),
        Projection::get_meters_per_pixel_at_latitude(0.0, MAX_ZOOM)
    );
}

// gtest: Projection.ProjectedMeters
#[test]
fn projected_meters() {
    let mut lat_lng = ll(0.0, 0.0);
    let mut projected_meters = Projection::projected_meters_for_lat_lng(&lat_lng);
    assert_eq!(projected_meters.northing(), projected_meters.easting());
    assert_eq!(lat_lng, Projection::lat_lng_for_projected_meters(&projected_meters));

    let south_west = ll(-LATITUDE_MAX, -LONGITUDE_MAX);
    projected_meters = Projection::projected_meters_for_lat_lng(&south_west);
    expect_double_eq!(projected_meters.northing(), -20_037_508.342_789_274);
    expect_double_eq!(projected_meters.easting(), -20_037_508.342_789_244);

    let north_east = ll(LATITUDE_MAX, LONGITUDE_MAX);
    projected_meters = Projection::projected_meters_for_lat_lng(&north_east);
    expect_double_eq!(projected_meters.northing(), 20_037_508.342_789_274);
    expect_double_eq!(projected_meters.easting(), 20_037_508.342_789_244);

    projected_meters = ProjectedMeters::new(f64::MIN, f64::MIN).unwrap();
    lat_lng = Projection::lat_lng_for_projected_meters(&projected_meters);
    assert_eq!(lat_lng.latitude(), -LATITUDE_MAX);
    assert_eq!(lat_lng.longitude(), -LONGITUDE_MAX);

    projected_meters = ProjectedMeters::new(f64::MAX, f64::MAX).unwrap();
    lat_lng = Projection::lat_lng_for_projected_meters(&projected_meters);
    assert_eq!(lat_lng.latitude(), LATITUDE_MAX);
    assert_eq!(lat_lng.longitude(), LONGITUDE_MAX);
}

// gtest: Projection.InvalidProjectedMeters
#[test]
fn invalid_projected_meters() {
    // C++ `ProjectedMeters{NAN}`: the easting defaults to 0.
    assert_eq!(ProjectedMeters::new(f64::NAN, 0.0).unwrap_err(), Error::invalid_argument("northing must not be NaN"));
    assert_eq!(ProjectedMeters::new(0.0, f64::NAN).unwrap_err(), Error::invalid_argument("easting must not be NaN"));
}

// Not in the C++ suite: the int-zoom `project` overload and the throwing
// `unproject` path.
#[test]
fn project_zoom_and_unproject_errors() {
    let p = Projection::project_zoom(&ll(0.0, 0.0), 1);
    let q = Projection::project(&ll(0.0, 0.0), 1.0 / TILE_SIZE_D * 2.0);
    assert_eq!(p, q);
    expect_double_eq!(p.x, 1.0);

    assert_eq!(
        Projection::unproject(&Point::new(f64::NAN, 0.0), 1.0, WrapMode::Unwrapped).unwrap_err(),
        Error::invalid_argument("longitude must not be NaN")
    );
    let wrapped = Projection::unproject(&Point::new(TILE_SIZE_D * 1.5, 256.0), 1.0, WrapMode::Wrapped).unwrap();
    // Unwrapped longitude 360 wraps to 0.
    assert_eq!(wrapped.longitude(), 0.0);
}
