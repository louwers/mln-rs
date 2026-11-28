use geometry::Point;
use mln_style::geo::{LatLng, LatLngBounds};
use mln_style::tile_id::CanonicalTileID;

use crate::geo::world::{WorldBox, WorldMapping};
use crate::tile::tile_range::TileRange;

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

fn c(z: u8, x: u32, y: u32) -> CanonicalTileID {
    CanonicalTileID::new(z, x, y)
}

fn bounds_from(id: CanonicalTileID) -> LatLngBounds {
    LatLngBounds::from_canonical_tile(id.z, id.x, id.y)
}

// gtest: TileRange.ContainsWorld
#[test]
fn contains_world() {
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::world(), 0);
    assert!(range.contains(&c(0, 0, 0)));
    assert!(!range.contains(&c(10, 0, 0)));
}

// gtest: TileRange.ContainsBoundsFromTile
#[test]
fn contains_bounds_from_tile() {
    {
        let bounds = bounds_from(c(3, 7, 0));
        let range = TileRange::from_lat_lng_bounds(&bounds, 3);
        assert!(range.contains(&c(3, 7, 0)));
    }
    {
        let bounds = bounds_from(c(10, 162, 395));
        let range = TileRange::from_lat_lng_bounds(&bounds, 10);
        assert!(range.contains(&c(10, 162, 395)));
    }
}

// gtest: TileRange.ContainsMultiZoom
#[test]
fn contains_multi_zoom() {
    let wrapped_bounds = LatLngBounds::hull(ll(37.6609, -122.5744), ll(37.8271, -122.3204));
    let range = TileRange::from_lat_lng_bounds_range(&wrapped_bounds, 5, 13);
    assert!(!range.contains(&c(0, 0, 0)));
    assert!(!range.contains(&c(5, 3, 11)));
    assert!(!range.contains(&c(6, 9, 22)));
    assert!(range.contains(&c(5, 5, 12)));
    assert!(range.contains(&c(6, 10, 24)));
    assert!(range.contains(&c(13, 1310, 3166)));
}

// gtest: TileRange.ContainsIntersectingTiles
#[test]
fn contains_intersecting_tiles() {
    let bounds = LatLngBounds::hull(ll(37.6609, -122.5744), ll(37.8271, -122.3204));
    let range = TileRange::from_lat_lng_bounds(&bounds, 13);
    assert!(!range.contains(&c(13, 1316, 3100)));
    assert!(range.contains(&c(13, 1310, 3166)));
}

// gtest: TileRange.ContainsWrappedBounds
#[test]
fn contains_wrapped_bounds() {
    let wrapped_bounds = LatLngBounds::hull(ll(37.6609, 237.6796), ll(37.8271, 237.4256));
    let range = TileRange::from_lat_lng_bounds(&wrapped_bounds, 13);
    assert!(!range.contains(&c(13, 1316, 3100)));
    assert!(range.contains(&c(13, 1310, 3166)));
}

// gtest: TileRange.ContainsBoundsCrossingAntimeridian
#[test]
fn contains_bounds_crossing_antimeridian() {
    {
        let crossing_bounds = LatLngBounds::hull(ll(-20.9615, -214.309), ll(19.477, -155.830));
        let range = TileRange::from_lat_lng_bounds(&crossing_bounds, 1);
        assert!(range.contains(&c(1, 1, 1)));
        assert!(range.contains(&c(1, 0, 0)));
    }
    {
        let crossing_bounds = LatLngBounds::hull(ll(-20.9615, -214.309), ll(19.477, -155.830));
        let range = TileRange::from_lat_lng_bounds(&crossing_bounds, 6);
        assert!(!range.contains(&c(6, 55, 34)));
        assert!(!range.contains(&c(6, 5, 28)));
        assert!(range.contains(&c(6, 63, 28)));
    }
}

// gtest: TileRange.ContainsUnwrappedWorld
#[test]
fn contains_unwrapped_world() {
    // The whole world, measured from the prime meridian.
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(-90.0, 0.0), ll(90.0, 360.0)), 2);
    for x in 0..4 {
        assert!(range.contains(&c(2, x, 1)), "{x}");
    }
}

// gtest: TileRange.ContainsBoundsWiderThanTheWorld
#[test]
fn contains_bounds_wider_than_the_world() {
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(10.0, 10.0), ll(20.0, 375.0)), 2);
    for x in 0..4 {
        assert!(range.contains(&c(2, x, 1)), "{x}");
    }
}

// gtest: TileRange.ContainsBoundsReachingEveryColumn
#[test]
fn contains_bounds_reaching_every_column() {
    // Less than a world wide, but it starts in column 2 and ends in column 2
    // one world east, so it touches every column.
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(10.0, 10.0), ll(20.0, 365.0)), 2);
    for x in 0..4 {
        assert!(range.contains(&c(2, x, 1)), "{x}");
    }
}

// gtest: TileRange.ContainsBoundsOneWorldWest
#[test]
fn contains_bounds_one_world_west() {
    // The same column as {{10, 0}, {20, 10}}.
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(10.0, -360.0), ll(20.0, -350.0)), 2);
    assert!(range.contains(&c(2, 2, 1)));
    assert!(!range.contains(&c(2, 1, 1)));
    assert!(!range.contains(&c(2, 3, 1)));
    assert!(!range.contains(&c(2, 0, 1)));
}

// gtest: TileRange.ContainsBoundsWhoseWidthOverflows
#[test]
fn contains_bounds_whose_width_overflows() {
    // Both edges are finite, but the width is not.
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(10.0, -1e308), ll(20.0, 1e308)), 2);
    for x in 0..4 {
        assert!(range.contains(&c(2, x, 1)), "{x}");
    }
}

// gtest: TileRange.ContainsHugeLongitude
#[test]
fn contains_huge_longitude() {
    // A whole number of worlds east of the prime meridian, so column 4 of 8.
    // 180 more degrees round away at this magnitude.
    let longitude = 360.0 * 2.0f64.powf(52.0);
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::hull(ll(10.0, longitude), ll(20.0, longitude)), 3);
    assert!(range.contains(&c(3, 4, 3)));
    assert!(!range.contains(&c(3, 5, 3)));
}

// gtest: TileRange.InvertedBoundsStayInTheWorld
#[test]
fn inverted_bounds_stay_in_the_world() {
    // Empty bounds are inverted: their east edge is west of their west edge.
    let range = TileRange::from_lat_lng_bounds(&LatLngBounds::empty(), 2);
    assert!(range.range.max.x < 4);
    assert!(!range.contains(&c(2, 0, 1)));
}

#[test]
fn contains_bounds_ending_at_the_antimeridian() {
    // Not a C++ test: the regression ffi patch 0076 has
    // (maplibre-native-ffi#1759), bounds that end at 180° reaching column 0.
    for bounds in
        [LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 180.0)), LatLngBounds::hull(ll(-10.0, -190.0), ll(10.0, -180.0))]
    {
        let range = TileRange::from_lat_lng_bounds(&bounds, 2);
        assert!(range.contains(&c(2, 3, 1)), "{bounds:?}");
        assert!(!range.contains(&c(2, 0, 1)), "{bounds:?}");
    }
}

#[test]
fn contains_bounds_starting_just_under_the_antimeridian() {
    // Not a C++ test: a west edge just under 180° projects to 180°, the
    // column past the last.
    let west = f64::from_bits(180.0f64.to_bits() - 1);
    let bounds = LatLngBounds::hull(ll(-10.0, west), ll(10.0, 180.0));
    let range = TileRange::from_lat_lng_bounds(&bounds, 2);
    assert!(range.contains(&c(2, 3, 1)));
    for x in 0..3 {
        assert!(!range.contains(&c(2, x, 1)), "column {x}");
    }
    let range = TileRange::from_lat_lng_bounds_range(&bounds, 1, 2);
    assert!(range.contains(&c(1, 1, 0)));
    assert!(!range.contains(&c(1, 0, 0)));
}

/// GL JS's CRS rotated by 30 degrees, whose world x and y both turn with
/// longitude and latitude.
fn rotated() -> WorldMapping {
    let (sin, cos) = (std::f64::consts::PI / 6.0).sin_cos();
    crate::tests::crs::mapping(crate::tests::crs::crs(
        "test:rotated",
        move |[lng, lat]| [lng * cos - lat * sin, lng * sin + lat * cos],
        move |[x, y]| [x * cos + y * sin, -x * sin + y * cos],
        [-150.0, 150.0],
        300.0,
    ))
}

/// The tile at zoom `z` of the place of `lat_lng` in `world`.
fn tile_of(world: &WorldMapping, lat_lng: LatLng, z: u8) -> CanonicalTileID {
    let point = world.world_from_lat_lng(&lat_lng);
    let tiles = 2.0f64.powi(i32::from(z));
    c(z, (point.x * tiles).floor() as u32, (point.y * tiles).floor() as u32)
}

/// The tiles from `min_zoom` to `max_zoom` of the rectangle `bounds` make
/// in the world square of `world`.
fn range_in(world: &WorldMapping, bounds: &LatLngBounds, min_zoom: u8, max_zoom: u8) -> TileRange {
    TileRange::from_world_box(&world.lng_lat_box_to_world_box(bounds), min_zoom, max_zoom)
}

fn world_box(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> WorldBox {
    WorldBox { min: Point::new(min_x, min_y), max: Point::new(max_x, max_y) }
}

// gtest: TileRange.FromWorldBox
#[test]
fn from_world_box() {
    // Lng 0..90 and lat 45..90 of the identity: the top row of the north-east
    // quarter.
    let range = TileRange::from_world_box(&world_box(0.5, 0.0, 1.0, 0.25), 0, 2);
    assert!(range.contains(&c(2, 2, 0)));
    assert!(range.contains(&c(2, 3, 0)));
    assert!(!range.contains(&c(2, 2, 1)));
    assert!(!range.contains(&c(2, 1, 0)));
    assert!(range.contains(&c(1, 1, 0)));
    assert!(!range.contains(&c(1, 0, 0)));
    assert!(range.contains(&c(0, 0, 0)));
}

// gtest: TileRange.FromWorldBoxOutsideTheSquare
#[test]
fn from_world_box_outside_the_square() {
    let range = TileRange::from_world_box(&world_box(1.5, 0.0, 2.0, 1.0), 0, 2);
    assert!(!range.contains(&c(0, 0, 0)));
    assert!(!range.contains(&c(2, 3, 0)));
}

#[test]
fn a_world_box_as_thin_as_a_line_covers_the_tiles_it_lies_in() {
    // On the edge between columns 1 and 2, it lies in column 2, as a
    // Mercator range's west edge does.
    let range = TileRange::from_world_box(&world_box(0.5, 0.25, 0.5, 0.75), 2, 2);
    for y in 1..3 {
        assert!(range.contains(&c(2, 2, y)), "row {y}");
        assert!(!range.contains(&c(2, 1, y)), "row {y}");
        assert!(!range.contains(&c(2, 0, y)), "row {y}");
    }
}

// gtest: TileRange.FromTheWorldBoxOfThePolarCapNorthOf60
#[test]
fn from_the_world_box_of_the_polar_cap_north_of_60() {
    let polar = crate::tests::crs::mapping(crate::tests::crs::polar_stereographic("test:polar"));
    let range = range_in(&polar, &LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0)), 2, 2);
    for x in 0..4 {
        for y in 0..4 {
            assert!(range.contains(&c(2, x, y)), "{x},{y}");
        }
    }
}

// gtest: TileRange.FromTheWorldBoxOfAPolarCapThatNoSampleReachesTheEdgeOf
#[test]
fn from_the_world_box_of_a_polar_cap_that_no_sample_reaches_the_edge_of() {
    // 45 E is the rightmost point of the 60 N circle, between two of the
    // box's longitude samples.
    let polar = crate::tests::crs::mapping(crate::tests::crs::polar_stereographic("test:polar"));
    let range = range_in(&polar, &LatLngBounds::hull(ll(60.0, -170.0), ll(90.0, 170.0)), 13, 13);
    assert!(range.contains(&tile_of(&polar, ll(60.0, 45.0), 13)));
}

// GL JS: TileBounds.contains follows the helper, not mercator.
#[test]
fn a_world_box_range_follows_the_projection_not_mercator() {
    let rotated = rotated();
    let bounds = LatLngBounds::hull(ll(40.0, 100.0), ll(50.0, 110.0));
    let in_rotated = range_in(&rotated, &bounds, 3, 3);
    let in_mercator = TileRange::from_lat_lng_bounds(&bounds, 3);

    let rotated_tile = tile_of(&rotated, ll(45.0, 105.0), 3);
    let mercator_tile = tile_of(&WorldMapping::Mercator, ll(45.0, 105.0), 3);
    assert_ne!(rotated_tile.x, mercator_tile.x);
    assert!(in_rotated.contains(&rotated_tile));
    assert!(!in_rotated.contains(&mercator_tile));
    assert!(in_mercator.contains(&mercator_tile));
    assert!(!in_mercator.contains(&rotated_tile));
}

// GL JS: TileBounds.contains covers every tile touched by the corners of a
// box in a non-cylindrical helper.
#[test]
fn a_world_box_range_holds_the_tiles_of_the_corners_of_a_turned_box() {
    let rotated = rotated();
    let range = range_in(&rotated, &LatLngBounds::hull(ll(-10.0, -10.0), ll(10.0, 10.0)), 5, 5);
    for [lng, lat] in [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]] {
        assert!(range.contains(&tile_of(&rotated, ll(lat, lng), 5)), "{lng}, {lat}");
    }
}
