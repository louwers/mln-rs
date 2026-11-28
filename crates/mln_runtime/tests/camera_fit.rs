//! The cameras that fit bounds, coordinates and geometries in a map's view.

use geo_types::{Geometry, GeometryCollection, LineString, MultiPoint, Point, Polygon};
use mln_engine::camera::{BoundOptions, CameraOptions};
use mln_engine::geo::EdgeInsets;
use mln_runtime::{CameraFitArea, CameraFitOptions, ErrorKind, Map};
use mln_style::geo::{LatLng, LatLngBounds};

use crate::support::still::{assert_double_eq, assert_near};
use crate::support::{map, runtime, wait, wait_err};

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

/// A map whose camera is over New York at zoom 16, and bounds over India and
/// China, far from it.
fn new_york(map: &Map) -> LatLngBounds {
    let camera = CameraOptions { center: Some(ll(40.712_730, -74.005_953)), zoom: Some(16.0), ..Default::default() };
    let update = mln_engine::camera::CameraUpdate { camera, ..Default::default() };
    wait(map.update_camera(update));
    LatLngBounds::hull(ll(15.68169, 73.499_857), ll(53.560_711, 134.77281))
}

/// Three places in India and China.
fn lat_lngs() -> Vec<LatLng> {
    vec![ll(40.712_730, 74.005_953), ll(15.68169, 73.499_857), ll(30.82678, 83.4082)]
}

fn fit(bearing: Option<f64>, pitch: Option<f64>) -> CameraFitOptions {
    CameraFitOptions { bearing, pitch, ..CameraFitOptions::default() }
}

/// A camera fits bounds and coordinates at the zoom and center that C++
/// finds, with the bearing and pitch asked for, within the map's pitch
/// limits.
#[test]
fn a_camera_fits_bounds_and_coordinates() {
    struct Case {
        name: &'static str,
        /// The bounds over New York's camera, else [`lat_lngs`].
        bounds: bool,
        /// The map's pitch limits.
        pitch_limits: Option<(f64, f64)>,
        bearing: Option<f64>,
        pitch: Option<f64>,
        zoom: f64,
        /// The center, for coordinates: the bounds' only needs to be in them.
        center: Option<(f64, f64)>,
        fitted_pitch: Option<f64>,
    }
    let cases = [
        Case {
            name: "lat_lng_bounds_to_camera",
            bounds: true,
            pitch_limits: None,
            bearing: None,
            pitch: None,
            zoom: 1.55467,
            center: None,
            fitted_pitch: None,
        },
        Case {
            name: "lat_lng_bounds_to_camera_with_bearing",
            bounds: true,
            pitch_limits: None,
            bearing: Some(35.0),
            pitch: None,
            zoom: 1.21385,
            center: None,
            fitted_pitch: None,
        },
        Case {
            name: "lat_lng_bounds_to_camera_with_bearing_and_pitch",
            bounds: true,
            pitch_limits: None,
            bearing: Some(35.0),
            pitch: Some(20.0),
            zoom: 13.66272,
            center: None,
            fitted_pitch: Some(20.0),
        },
        Case {
            name: "lat_lngs_to_camera",
            bounds: false,
            pitch_limits: None,
            bearing: Some(23.0),
            pitch: None,
            zoom: 2.75434,
            center: Some((28.49288, 74.97437)),
            fitted_pitch: None,
        },
        Case {
            name: "lat_lngs_to_camera_with_bearing_and_pitch",
            bounds: false,
            pitch_limits: None,
            bearing: Some(23.0),
            pitch: Some(20.0),
            zoom: 3.04378,
            center: Some((28.53718, 74.31746)),
            fitted_pitch: Some(20.0),
        },
        // The pitch limits clamp the pitch asked for, and the fit is the
        // one at the clamped pitch.
        Case {
            name: "lat_lngs_to_camera_with_bearing_and_pitch_max",
            bounds: false,
            pitch_limits: Some((0.0, 0.0)),
            bearing: Some(23.0),
            pitch: Some(45.0),
            zoom: 2.75434,
            center: Some((28.49288, 74.97437)),
            fitted_pitch: Some(0.0),
        },
        Case {
            name: "lat_lngs_to_camera_with_bearing_and_pitch_min",
            bounds: false,
            pitch_limits: Some((20.0, 60.0)),
            bearing: Some(23.0),
            pitch: Some(0.0),
            zoom: 3.04378,
            center: Some((28.53718, 74.31746)),
            fitted_pitch: Some(20.0),
        },
    ];

    let runtime = runtime();
    for case in cases {
        let name = case.name;
        let map = map(&runtime);
        if let Some((min_pitch, max_pitch)) = case.pitch_limits {
            let limits = BoundOptions { min_pitch: Some(min_pitch), max_pitch: Some(max_pitch), ..Default::default() };
            wait(map.set_bounds(limits));
            let bounds = map.snapshot().unwrap().bounds;
            assert_double_eq(bounds.min_pitch, min_pitch);
            assert_double_eq(bounds.max_pitch, max_pitch);
        }
        let (area, bounds) = if case.bounds {
            let bounds = new_york(&map);
            (CameraFitArea::Bounds(bounds), Some(bounds))
        } else {
            (CameraFitArea::Coordinates(lat_lngs()), None)
        };

        let camera = wait(map.camera_for(area, fit(case.bearing, case.pitch)));
        let center = camera.center.unwrap();
        assert_near(camera.zoom.unwrap(), case.zoom, 1e-5);
        assert_near(camera.bearing.unwrap_or(0.0), case.bearing.unwrap_or(0.0), 1e-5);
        if let Some(bounds) = bounds {
            assert!(bounds.contains(&center), "{name}: {center:?}");
        }
        if let Some((lat, lon)) = case.center {
            assert_near(center.latitude(), lat, 1e-5);
            assert_near(center.longitude(), lon, 1e-5);
        }
        if let Some(pitch) = case.fitted_pitch {
            assert_double_eq(camera.pitch.unwrap(), pitch);
        }
        wait(map.release());
    }
}

/// Padding keeps a fit's center, and zooms it out by as much as the padding
/// shrinks the view along its narrower side.
#[test]
fn padding_zooms_a_fit_out() {
    let runtime = runtime();
    let map = map(&runtime);
    let bounds = new_york(&map);

    let camera = wait(map.camera_for(CameraFitArea::Bounds(bounds), fit(Some(35.0), Some(20.0))));
    let padding = EdgeInsets::new(10.0, 20.0, 30.0, 40.0);
    let options = CameraFitOptions { padding, ..fit(Some(35.0), Some(20.0)) };
    let padded = wait(map.camera_for(CameraFitArea::Bounds(bounds), options));
    assert!(bounds.contains(&padded.center.unwrap()));
    assert_double_eq(padded.center.unwrap().latitude(), camera.center.unwrap().latitude());
    assert_double_eq(padded.center.unwrap().longitude(), camera.center.unwrap().longitude());

    let size = map.snapshot().unwrap().view.extent.size;
    let (width, height) = (f64::from(size.width), f64::from(size.height));
    let scale_change =
        ((width - padding.left() - padding.right()) / width).min((height - padding.top() - padding.bottom()) / height);
    assert_double_eq(padded.zoom.unwrap(), camera.zoom.unwrap() + scale_change.log2());
    assert_double_eq(padded.pitch.unwrap(), camera.pitch.unwrap());
    assert_double_eq(padded.bearing.unwrap(), camera.bearing.unwrap());
}

/// A geometry fits as its points do, wherever they sit in it, and one with a
/// point at no latitude fits nowhere.
#[test]
fn geometry_to_camera() {
    let runtime = runtime();
    let map = map(&runtime);
    let points: LineString<f64> = lat_lngs().iter().map(|ll| (ll.longitude(), ll.latitude())).collect();
    let empty = Geometry::GeometryCollection(GeometryCollection(Vec::new()));
    let geometry = Geometry::GeometryCollection(GeometryCollection(vec![
        Geometry::Point(points.0[0].into()),
        Geometry::Polygon(Polygon::new(LineString(vec![points.0[1], points.0[2], points.0[1]]), vec![])),
        empty,
    ]));

    let options = fit(Some(23.0), Some(20.0));
    let expected = wait(map.camera_for(CameraFitArea::Coordinates(lat_lngs()), options));
    let fitted = wait(map.camera_for(CameraFitArea::Geometry(geometry), options));
    assert_double_eq(fitted.zoom.unwrap(), expected.zoom.unwrap());
    assert_double_eq(fitted.center.unwrap().latitude(), expected.center.unwrap().latitude());
    assert_double_eq(fitted.center.unwrap().longitude(), expected.center.unwrap().longitude());
    assert_double_eq(fitted.bearing.unwrap(), 23.0);
    assert_double_eq(fitted.pitch.unwrap(), 20.0);

    let fitted = wait(map.camera_for(CameraFitArea::Geometry(Geometry::LineString(points)), fit(None, None)));
    let expected = wait(map.camera_for(CameraFitArea::Coordinates(lat_lngs()), fit(None, None)));
    assert_double_eq(fitted.zoom.unwrap(), expected.zoom.unwrap());

    let beyond_the_pole = Geometry::MultiPoint(MultiPoint(vec![Point::new(0.0, 0.0), Point::new(0.0, 91.0)]));
    let error = wait_err(map.camera_for(CameraFitArea::Geometry(beyond_the_pole), fit(None, None)));
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{error:?}");
}
