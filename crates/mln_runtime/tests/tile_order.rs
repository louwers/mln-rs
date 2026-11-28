//! Where circles of two tiles overlap, the frame's order of its tiles stacks
//! them, whatever views came before.

use mln_engine::camera::CameraOptions;
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;

use crate::support::still::{StillMap, center_pixel};

const ZOOM: f64 = 3.0;
/// The latitude of the views' centers, inside a row of tiles of zoom 3,
/// whose 512 pixel tiles are each twice as wide as a view.
const LATITUDE: f64 = 30.0;

/// The longitude `pixels` east of the meridian, at [`ZOOM`].
fn longitude(pixels: f64) -> f64 {
    pixels * 360.0 / (512.0 * 2f64.powf(ZOOM))
}

/// Two circles, blue just west of the meridian and red just east of it,
/// each in its own tile, which overlap on the meridian.
fn style() -> String {
    let point = |lng: f64, color: &str| {
        format!(
            r#"{{"type": "Feature", "properties": {{"color": "{color}"}},
                "geometry": {{"type": "Point", "coordinates": [{lng}, {LATITUDE}]}}}}"#
        )
    };
    let (blue, red) = (point(longitude(-4.0), "blue"), point(longitude(4.0), "red"));
    format!(
        r#"{{
            "version": 8,
            "sources": {{
                "points": {{
                    "type": "geojson",
                    "maxzoom": 3,
                    "buffer": 0,
                    "data": {{"type": "FeatureCollection", "features": [{blue}, {red}]}}
                }}
            }},
            "layers": [
                {{"id": "background", "type": "background", "paint": {{"background-color": "white"}}}},
                {{
                    "id": "circles",
                    "type": "circle",
                    "source": "points",
                    "paint": {{"circle-color": ["get", "color"], "circle-radius": 10}}
                }}
            ]
        }}"#
    )
}

/// The still of the map of [`style`] seen from each longitude of `centers`
/// in turn: the last one.
fn render(centers: &[f64]) -> PremultipliedImage {
    let map = StillMap::still();
    map.load_json(&style());
    let mut image = None;
    for &lng in centers {
        let center = LatLng::new(LATITUDE, lng).unwrap();
        map.jump(CameraOptions { center: Some(center), zoom: Some(ZOOM), ..Default::default() });
        image = Some(map.render());
    }
    image.expect("a center")
}

/// The overlap of the circles shows the same circle in a fresh view of the
/// meridian as after a view of only the west tile, or only the east one.
#[test]
fn circles_of_two_tiles_stack_alike_however_the_view_is_reached() {
    let (west, east) = (longitude(-256.0), longitude(256.0));
    let overlaps = [&[0.0][..], &[west, 0.0], &[east, 0.0]].map(|centers| center_pixel(&render(centers)));
    for &[r, g, b, _] in &overlaps {
        assert!(
            (r > 200 && g < 60 && b < 60) || (r < 60 && g < 60 && b > 200),
            "a circle at the overlap, not {:?}",
            [r, g, b]
        );
    }
    assert_eq!(overlaps[0], overlaps[1], "fresh, then after the west tile");
    assert_eq!(overlaps[0], overlaps[2], "fresh, then after the east tile");
}
