use std::cell::Cell;
use std::rc::Rc;

use geometry::Point;
use mln_map::transform::{Transform, TransformObserver};
use mln_map::{CameraChange, CameraChangeMode};
use mln_map_types::tile_coordinate::TileCoordinate;
use mln_map_types::{ConstrainMode, ViewportMode};
use mln_tile_id::{CanonicalTileID, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::size::Size;

/// gtest's `ASSERT_DOUBLE_EQ`: equal within 4 ULPs.
fn assert_double_eq(actual: f64, expected: f64) {
    let ulps = |x: f64| -> i64 {
        let bits = x.to_bits() as i64;
        if bits < 0 { i64::MIN - bits } else { bits }
    };
    assert!(actual == expected || (ulps(actual) - ulps(expected)).abs() <= 4, "{actual} != {expected}");
}

fn assert_near(actual: f64, expected: f64, abs_error: f64) {
    assert!((actual - expected).abs() <= abs_error, "|{actual} - {expected}| > {abs_error}");
}

struct Observer {
    change_count: Rc<Cell<usize>>,
}

impl TransformObserver for Observer {
    fn on_camera_will_change(&self, change: CameraChange) {
        if change.mode == CameraChangeMode::Immediate {
            assert_eq!(self.change_count.get(), 0);
            self.change_count.set(self.change_count.get() + 1);
        }
    }

    fn on_camera_did_change(&self, change: CameraChange) {
        if change.mode == CameraChangeMode::Immediate {
            assert_eq!(self.change_count.get(), 1);
        }
    }
}

// gtest: TileCoordinate.FromLatLng
#[test]
fn from_lat_lng() {
    let change_count = Rc::new(Cell::new(0usize));
    let observer = Rc::new(Observer { change_count: Rc::clone(&change_count) });
    let mut transform = Transform::new(observer, ConstrainMode::HeightOnly, ViewportMode::Default);

    let max = constants::TILE_SIZE_D;
    transform.resize(Size::new(max as u32, max as u32)).unwrap();

    // Center, top-left, bottom-left, bottom-right, top-right edges.
    let edges: Vec<(LatLng, ScreenCoordinate)> = vec![
        (LatLng::default(), ScreenCoordinate::new(max / 2.0, max / 2.0)),
        (LatLng::new(constants::LATITUDE_MAX, -constants::LONGITUDE_MAX).unwrap(), ScreenCoordinate::new(0.0, max)),
        (LatLng::new(-constants::LATITUDE_MAX, -constants::LONGITUDE_MAX).unwrap(), ScreenCoordinate::new(0.0, 0.0)),
        (LatLng::new(-constants::LATITUDE_MAX, constants::LONGITUDE_MAX).unwrap(), ScreenCoordinate::new(max, 0.0)),
        (LatLng::new(constants::LATITUDE_MAX, constants::LONGITUDE_MAX).unwrap(), ScreenCoordinate::new(max, max)),
    ];

    for (lat_lng, screen_coordinate) in &edges {
        let base = TileCoordinate::from_lat_lng(0.0, lat_lng);

        // 16 is the maximum zoom level where we actually compute placements.
        for integer_zoom in 0u8..=16 {
            let zoom = f64::from(integer_zoom);
            let max_tiles_per_axis = 2.0f64.powf(zoom);
            let tile_point = Point::new(
                if lat_lng.longitude() == 0.0 {
                    0.5
                } else if lat_lng.longitude() == -constants::LONGITUDE_MAX {
                    0.0
                } else {
                    1.0
                },
                if lat_lng.latitude() == 0.0 {
                    0.5
                } else if lat_lng.latitude() == -constants::LATITUDE_MAX {
                    1.0
                } else {
                    0.0
                },
            );

            let from_lat_lng = TileCoordinate::from_lat_lng(zoom, lat_lng);
            assert_double_eq(from_lat_lng.z, zoom);
            assert_double_eq(from_lat_lng.p.x, tile_point.x * max_tiles_per_axis);
            assert_near(from_lat_lng.p.y, tile_point.y * max_tiles_per_axis, 1.0e-7);

            let from_screen_coordinate =
                TileCoordinate::from_screen_coordinate(transform.get_state(), zoom as u8, screen_coordinate);
            assert_double_eq(from_screen_coordinate.z, from_lat_lng.z);
            assert_near(from_screen_coordinate.p.x, from_lat_lng.p.x, 0.99);
            assert_near(from_screen_coordinate.p.y, from_lat_lng.p.y, 0.99);

            let zoomed = base.zoom_to(zoom);
            assert_double_eq(zoomed.z, zoom);
            assert_double_eq(zoomed.p.x, from_lat_lng.p.x);
            assert_double_eq(zoomed.p.y, from_lat_lng.p.y);
        }
    }
    assert_eq!(change_count.get(), 1);
}

// gtest: TileCoordinate.ToGeometryCoordinate
#[test]
fn to_geometry_coordinate() {
    let edges: Vec<Point<f64>> = vec![
        Point::new(0.5, 0.5),
        Point::new(0.0, 0.0),
        Point::new(1.0, 0.0),
        Point::new(1.0, 1.0),
        Point::new(0.0, 1.0),
    ];

    for zoom in 0u8..=16 {
        let max_tiles_per_axis = 2.0f64.powi(i32::from(zoom)) as u32;
        for edge in &edges {
            let tile_x: u32 = if edge.x == 0.0 {
                0
            } else if edge.x == 1.0 {
                max_tiles_per_axis - 1
            } else {
                ((f64::from(max_tiles_per_axis) / 2.0) - 1.0) as u32
            };
            let tile_y: u32 = if edge.y == 0.0 {
                0
            } else if edge.y == 1.0 {
                max_tiles_per_axis - 1
            } else {
                ((f64::from(max_tiles_per_axis) / 2.0) - 1.0) as u32
            };
            let unwrapped = UnwrappedTileID::with_wrap(0, CanonicalTileID::new(zoom, tile_x, tile_y));

            // C++: `(edge.x * maxTilesPerAxis) - tileX` is double arithmetic,
            // times the int EXTENT.
            let tile_point_x =
                ((edge.x * f64::from(max_tiles_per_axis)) - f64::from(tile_x)) * f64::from(constants::EXTENT);
            let tile_point_y =
                ((edge.y * f64::from(max_tiles_per_axis)) - f64::from(tile_y)) * f64::from(constants::EXTENT);
            let point = TileCoordinate::to_geometry_coordinate(&unwrapped, edge);
            assert_double_eq(f64::from(point.x), tile_point_x);
            assert_double_eq(f64::from(point.y), tile_point_y);
        }
    }
}
