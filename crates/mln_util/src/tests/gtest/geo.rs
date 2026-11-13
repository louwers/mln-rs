// C++ constructors that throw std::domain_error return `Err(message)`;
// `LatLng(CanonicalTileID)` / `LatLng(UnwrappedTileID)` /
// `LatLngBounds(CanonicalTileID)` are the `from_*_tile` functions (mln_util
// sits below mln_tile_id), called with the tile ids' fields.

use mln_tile_id::{CanonicalTileID, UnwrappedTileID};

use crate::Error;
use crate::constants::{LATITUDE_MAX, LONGITUDE_MAX};
use crate::geo::{EdgeInsets, LatLng, LatLngBounds, WrapMode};
use crate::tests::gtest::assert_double_eq;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

fn from_canonical(id: CanonicalTileID) -> LatLng {
    LatLng::from_canonical_tile(id.z, id.x, id.y)
}

fn from_unwrapped(id: UnwrappedTileID) -> LatLng {
    LatLng::from_unwrapped_tile(id.canonical.z, id.canonical.x, id.canonical.y, id.wrap)
}

fn bounds_from_canonical(id: CanonicalTileID) -> LatLngBounds {
    LatLngBounds::from_canonical_tile(id.z, id.x, id.y)
}

// gtest: LatLng.InvalidLatLng
#[test]
fn invalid_lat_lng() {
    assert_eq!(LatLng::new(f64::NAN, 0.0).unwrap_err(), Error::invalid_argument("latitude must not be NaN"));
    assert_eq!(LatLng::new(0.0, f64::NAN).unwrap_err(), Error::invalid_argument("longitude must not be NaN"));
    assert_eq!(LatLng::new(91.0, 0.0).unwrap_err(), Error::invalid_argument("latitude must be between -90 and 90"));
    assert_eq!(LatLng::new(0.0, f64::INFINITY).unwrap_err(), Error::invalid_argument("longitude must not be infinite"));
}

// gtest: EdgeInsets.InvalidEdgeInsets
#[test]
fn invalid_edge_insets() {
    assert_eq!(EdgeInsets::new(f64::NAN, 0.0, 0.0, 0.0).unwrap_err(), Error::invalid_argument("top must not be NaN"));
    assert_eq!(EdgeInsets::new(0.0, f64::NAN, 0.0, 0.0).unwrap_err(), Error::invalid_argument("left must not be NaN"));
    assert_eq!(
        EdgeInsets::new(0.0, 0.0, f64::NAN, 0.0).unwrap_err(),
        Error::invalid_argument("bottom must not be NaN")
    );
    assert_eq!(EdgeInsets::new(0.0, 0.0, 0.0, f64::NAN).unwrap_err(), Error::invalid_argument("right must not be NaN"));
}

// gtest: LatLngBounds.World
#[test]
fn world() {
    let result = LatLngBounds::world();
    assert_double_eq(-90.0, result.south());
    assert_double_eq(90.0, result.north());
    assert_double_eq(-180.0, result.west());
    assert_double_eq(180.0, result.east());
}

// gtest: LatLngBounds.Singleton
#[test]
fn singleton() {
    let result = LatLngBounds::singleton(ll(1.0, 2.0));
    assert_double_eq(1.0, result.south());
    assert_double_eq(1.0, result.north());
    assert_double_eq(2.0, result.west());
    assert_double_eq(2.0, result.east());
}

// gtest: LatLngBounds.Hull
#[test]
fn hull() {
    let (s, w, n, e) = (1.0, 2.0, 3.0, 4.0);

    let swne = LatLngBounds::hull(ll(s, w), ll(n, e));
    assert_double_eq(s, swne.south());
    assert_double_eq(n, swne.north());
    assert_double_eq(w, swne.west());
    assert_double_eq(e, swne.east());

    let nesw = LatLngBounds::hull(ll(n, e), ll(s, w));
    assert_double_eq(s, nesw.south());
    assert_double_eq(n, nesw.north());
    assert_double_eq(w, nesw.west());
    assert_double_eq(e, nesw.east());

    let senw = LatLngBounds::hull(ll(s, e), ll(n, w));
    assert_double_eq(s, senw.south());
    assert_double_eq(n, senw.north());
    assert_double_eq(w, senw.west());
    assert_double_eq(e, senw.east());

    let nwse = LatLngBounds::hull(ll(n, w), ll(s, e));
    assert_double_eq(s, nwse.south());
    assert_double_eq(n, nwse.north());
    assert_double_eq(w, nwse.west());
    assert_double_eq(e, nwse.east());
}

// gtest: LatLngBounds.Empty
#[test]
fn empty() {
    assert!(LatLngBounds::empty().is_empty());
    assert!(!LatLngBounds::world().is_empty());
}

// gtest: LatLngBounds.Center
#[test]
fn center() {
    let result = LatLngBounds::hull(ll(1.0, 2.0), ll(3.0, 4.0)).center();
    assert_double_eq(2.0, result.latitude());
    assert_double_eq(3.0, result.longitude());
}

// gtest: LatLngBounds.Southwest
#[test]
fn southwest() {
    let result = LatLngBounds::hull(ll(1.0, 2.0), ll(3.0, 4.0)).southwest();
    assert_double_eq(1.0, result.latitude());
    assert_double_eq(2.0, result.longitude());
}

// gtest: LatLngBounds.Northeast
#[test]
fn northeast() {
    let result = LatLngBounds::hull(ll(1.0, 2.0), ll(3.0, 4.0)).northeast();
    assert_double_eq(3.0, result.latitude());
    assert_double_eq(4.0, result.longitude());
}

// gtest: LatLngBounds.Southeast
#[test]
fn southeast() {
    let result = LatLngBounds::hull(ll(1.0, 2.0), ll(3.0, 4.0)).southeast();
    assert_double_eq(1.0, result.latitude());
    assert_double_eq(4.0, result.longitude());
}

// gtest: LatLngBounds.Northwest
#[test]
fn northwest() {
    let result = LatLngBounds::hull(ll(1.0, 2.0), ll(3.0, 4.0)).northwest();
    assert_double_eq(3.0, result.latitude());
    assert_double_eq(2.0, result.longitude());
}

// gtest: LatLng.FromTileID
#[test]
fn from_tile_id() {
    for i in 0..20 {
        let ll = from_canonical(CanonicalTileID::new(i, 0, 0));
        assert_double_eq(-LONGITUDE_MAX, ll.longitude());
        assert_double_eq(LATITUDE_MAX, ll.latitude());
    }

    {
        let ll = from_unwrapped(UnwrappedTileID::new(0, 1, 0));
        assert_double_eq(LONGITUDE_MAX, ll.longitude());
        assert_double_eq(LATITUDE_MAX, ll.latitude());
    }

    {
        let ll = from_unwrapped(UnwrappedTileID::new(0, -1, 0));
        assert_double_eq(-540.0, ll.longitude());
        assert_double_eq(LATITUDE_MAX, ll.latitude());
    }
}

// gtest: LatLng.Boundaries
#[test]
fn boundaries() {
    let mut coordinate = LatLng::default();
    assert_double_eq(0.0, coordinate.latitude());
    assert_double_eq(0.0, coordinate.longitude());

    coordinate = ll(0.0, -180.1);
    assert_double_eq(-180.1, coordinate.longitude());

    coordinate.wrap();
    assert_double_eq(179.9, coordinate.longitude()); // 1E-14

    coordinate = ll(0.0, 180.9);
    coordinate.wrap();
    assert_double_eq(-179.1, coordinate.longitude());

    coordinate = ll(0.0, -360.5);
    coordinate.wrap();
    assert_double_eq(-0.5, coordinate.longitude());

    coordinate = ll(0.0, 360.5);
    coordinate.wrap();
    assert_double_eq(0.5, coordinate.longitude());

    coordinate = ll(0.0, 360_000.5);
    coordinate.wrap();
    assert_double_eq(0.5, coordinate.longitude());
}

// gtest: LatLngBounds.FromTileID
#[test]
fn lat_lng_bounds_from_tile_id() {
    {
        let bounds = bounds_from_canonical(CanonicalTileID::new(0, 0, 0));
        assert_double_eq(-LONGITUDE_MAX, bounds.west());
        assert_double_eq(-LATITUDE_MAX, bounds.south());
        assert_double_eq(LONGITUDE_MAX, bounds.east());
        assert_double_eq(LATITUDE_MAX, bounds.north());
    }

    {
        let bounds = bounds_from_canonical(CanonicalTileID::new(1, 0, 1));
        assert_double_eq(-LONGITUDE_MAX, bounds.west());
        assert_double_eq(-LATITUDE_MAX, bounds.south());
        assert_double_eq(0.0, bounds.east());
        assert_double_eq(0.0, bounds.north());
    }

    {
        let bounds = bounds_from_canonical(CanonicalTileID::new(1, 1, 1));
        assert_double_eq(0.0, bounds.west());
        assert_double_eq(-LATITUDE_MAX, bounds.south());
        assert_double_eq(LONGITUDE_MAX, bounds.east());
        assert_double_eq(0.0, bounds.north());
    }

    {
        let bounds = bounds_from_canonical(CanonicalTileID::new(1, 0, 0));
        assert_double_eq(-LONGITUDE_MAX, bounds.west());
        assert_double_eq(0.0, bounds.south());
        assert_double_eq(0.0, bounds.east());
        assert_double_eq(LATITUDE_MAX, bounds.north());
    }
}

// gtest: LatLngBounds.ContainsPoint
#[test]
fn contains_point() {
    let bounds = LatLngBounds::hull(ll(50.0, -100.0), ll(-50.0, 100.0));

    assert!(!bounds.contains(&ll(0.0, 170.0)));
    assert!(!bounds.contains(&ll(0.0, -170.0)));
    assert!(bounds.contains(&ll(0.0, -100.0)));
    assert!(bounds.contains(&ll(-50.0, 100.0)));
    assert!(!bounds.contains(&ll(0.0, 365.0)));
}

// gtest: LatLngBounds.ContainsPoint_Wrapped
#[test]
fn contains_point_wrapped() {
    let mut bounds = LatLngBounds::hull(ll(50.0, -160.0), ll(-50.0, 160.0));
    assert!(!bounds.contains(&ll(0.0, 170.0)));
    assert!(!bounds.contains(&ll(0.0, -170.0)));

    bounds = LatLngBounds::hull(ll(50.0, -200.0), ll(-50.0, -160.0));
    assert!(!bounds.contains(&ll(0.0, 170.0)));
    assert!(bounds.contains_with_mode(&ll(0.0, 170.0), WrapMode::Wrapped));
    assert!(bounds.contains(&ll(0.0, -170.0)));
    assert!(bounds.contains_with_mode(&ll(0.0, -170.0), WrapMode::Wrapped));
    assert!(!bounds.contains(&ll(0.0, 190.0)));
    assert!(bounds.contains_with_mode(&ll(0.0, 190.0), WrapMode::Wrapped));
    assert!(!bounds.contains(&ll(0.0, 541.0)));
    assert!(bounds.contains_with_mode(&ll(0.0, 541.0), WrapMode::Wrapped));
}

// gtest: LatLngBounds.ContainsBounds
#[test]
fn contains_bounds() {
    let bounds = LatLngBounds::hull(ll(50.0, -160.0), ll(-50.0, 160.0));
    assert!(bounds.contains_bounds(&bounds, WrapMode::Unwrapped));

    let mut inner_bounds = LatLngBounds::hull(ll(10.0, -180.0), ll(-10.0, -170.0));
    assert!(!bounds.contains_bounds(&inner_bounds, WrapMode::Unwrapped));
    assert!(!inner_bounds.contains_bounds(&bounds, WrapMode::Unwrapped));

    inner_bounds = LatLngBounds::hull(ll(10.0, 120.0), ll(-60.0, 125.0));
    assert!(!bounds.contains_bounds(&inner_bounds, WrapMode::Unwrapped));
    assert!(!inner_bounds.contains_bounds(&bounds, WrapMode::Unwrapped));

    inner_bounds = LatLngBounds::hull(ll(10.0, 120.0), ll(-10.0, 125.0));
    assert!(bounds.contains_bounds(&inner_bounds, WrapMode::Unwrapped));
    assert!(!inner_bounds.contains_bounds(&bounds, WrapMode::Unwrapped));
}

// gtest: LatLngBounds.ContainsBounds_Wrapped
#[test]
fn contains_bounds_wrapped() {
    let bounds = LatLngBounds::hull(ll(50.0, -200.0), ll(-50.0, -160.0));

    let mut inner = LatLngBounds::hull(ll(10.0, -180.0), ll(-10.0, -170.0));
    assert!(bounds.contains_bounds(&inner, WrapMode::Unwrapped));
    assert!(bounds.contains_bounds(&inner, WrapMode::Wrapped));

    inner = LatLngBounds::hull(ll(10.0, 180.0), ll(-10.0, 190.0));
    assert!(!bounds.contains_bounds(&inner, WrapMode::Unwrapped));
    assert!(bounds.contains_bounds(&inner, WrapMode::Wrapped));

    inner = LatLngBounds::hull(ll(10.0, 190.0), ll(-10.0, 220.0));
    assert!(!bounds.contains_bounds(&inner, WrapMode::Unwrapped));
    assert!(!bounds.contains_bounds(&inner, WrapMode::Wrapped));

    let mut unwrapped = LatLngBounds::hull(ll(10.0, 170.0), ll(-10.0, -175.0));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Unwrapped));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Wrapped));

    unwrapped = LatLngBounds::hull(ll(10.0, 0.0), ll(-10.0, -10.0));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Unwrapped));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Wrapped));

    unwrapped = LatLngBounds::hull(ll(10.0, -165.0), ll(-10.0, -180.0));
    assert!(bounds.contains_bounds(&unwrapped, WrapMode::Unwrapped));
    assert!(bounds.contains_bounds(&unwrapped, WrapMode::Wrapped));

    unwrapped = LatLngBounds::hull(ll(10.0, 180.0), ll(-10.0, 160.0));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Unwrapped));
    assert!(bounds.contains_bounds(&unwrapped, WrapMode::Wrapped));

    unwrapped = LatLngBounds::hull(ll(10.0, 540.0), ll(-10.0, 560.0));
    assert!(!bounds.contains_bounds(&unwrapped, WrapMode::Unwrapped));
    assert!(bounds.contains_bounds(&unwrapped, WrapMode::Wrapped));
}

// gtest: LatLngBounds.ContainsTileIDs
#[test]
fn contains_tile_ids() {
    let bounds = bounds_from_canonical(CanonicalTileID::new(4, 2, 6));
    let inner_bounds = bounds_from_canonical(CanonicalTileID::new(9, 82, 197));
    assert!(bounds.contains_bounds(&inner_bounds, WrapMode::Unwrapped));
    assert!(!bounds.contains_bounds(&bounds_from_canonical(CanonicalTileID::new(3, 1, 0)), WrapMode::Unwrapped));
}

// gtest: LatLngBounds.Intersects
#[test]
fn intersects() {
    let bounds = LatLngBounds::hull(ll(50.0, -160.0), ll(-50.0, 160.0));
    assert!(bounds.intersects(bounds, WrapMode::Unwrapped));

    let other = LatLngBounds::hull(ll(50.0, -160.0), ll(10.0, 160.0));
    assert!(bounds.intersects(other, WrapMode::Unwrapped));
    assert!(other.intersects(bounds, WrapMode::Unwrapped));
}

// gtest: LatLngBounds.Intersects_Wrapped
#[test]
fn intersects_wrapped() {
    let bounds = LatLngBounds::hull(ll(50.0, -200.0), ll(-50.0, -160.0));
    assert!(bounds.intersects(bounds, WrapMode::Unwrapped));

    let mut other = LatLngBounds::hull(ll(50.0, -150.0), ll(10.0, 160.0));
    assert!(!bounds.intersects(other, WrapMode::Unwrapped));
    assert!(!other.intersects(bounds, WrapMode::Unwrapped));
    assert!(!bounds.intersects(other, WrapMode::Wrapped));
    assert!(!other.intersects(bounds, WrapMode::Wrapped));

    other = LatLngBounds::hull(ll(10.0, -150.0), ll(-10.0, -210.0));
    assert!(bounds.intersects(other, WrapMode::Unwrapped));
    assert!(bounds.intersects(other, WrapMode::Wrapped));
    assert!(other.intersects(bounds, WrapMode::Unwrapped));
    assert!(other.intersects(bounds, WrapMode::Wrapped));

    other = LatLngBounds::hull(ll(10.0, 150.0), ll(-10.0, 210.0));
    assert!(!bounds.intersects(other, WrapMode::Unwrapped));
    assert!(!other.intersects(bounds, WrapMode::Unwrapped));
    assert!(bounds.intersects(other, WrapMode::Wrapped));
    assert!(other.intersects(bounds, WrapMode::Wrapped));

    other = LatLngBounds::hull(ll(10.0, 195.0), ll(-10.0, 300.0));
    assert!(!bounds.intersects(other, WrapMode::Unwrapped));
    assert!(!other.intersects(bounds, WrapMode::Unwrapped));
    assert!(bounds.intersects(other, WrapMode::Wrapped));
    assert!(other.intersects(bounds, WrapMode::Wrapped));
}
