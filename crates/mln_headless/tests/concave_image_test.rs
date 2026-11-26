// An image source whose corners make a concave quad draws inside its outline,
// whichever corner points into it.

use std::sync::Arc;

use mln_engine::camera::CameraOptions;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_util::geo::ScreenCoordinate;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

const SIDE: u32 = 256;

/// A still of an opaque white image on `coordinates` (longitude, latitude of
/// the top left, top right, bottom right and bottom left corners) over
/// nothing, and the corners on screen.
fn render(coordinates: [[f64; 2]; 4]) -> (PremultipliedImage, [ScreenCoordinate; 4]) {
    let (queue, pool) = (Queue::new(), Pool::new(4));
    let size = Size::new(SIDE, SIDE);
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let mut white = PremultipliedImage::new(Size::new(8, 8));
    white.data.fill(255);
    let png = Arc::new(mln_engine::image::encode_png(&white));
    file_source.set_response(Kind::Image, move |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&png));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
    frontend.attach(&mut map);
    let json = coordinates.map(|[lng, lat]| format!("[{lng}, {lat}]")).join(", ");
    map.load_style_json(&format!(
        r#"{{
            "version": 8,
            "sources": {{"image": {{"type": "image", "url": "image.png", "coordinates": [{json}]}}}},
            "layers": [{{"id": "image", "type": "raster", "source": "image", "paint": {{"raster-fade-duration": 0}}}}]
        }}"#
    ));
    let center = LatLng::new(0.0, 0.0).unwrap();
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(1.5), ..Default::default() });
    let image = frontend.render(&mut map).expect("the still").image;
    (image, coordinates.map(|[lng, lat]| map.pixel_for_lat_lng(&LatLng::new(lat, lng).unwrap())))
}

/// Whether `point` is inside the outline of `corners`.
fn inside(corners: &[ScreenCoordinate; 4], point: (f64, f64)) -> bool {
    let mut inside = false;
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        if (a.y > point.1) != (b.y > point.1) && point.0 < a.x + (point.1 - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

#[test]
fn a_concave_image_draws_inside_its_outline() {
    // The bottom left corner, then the bottom right, pulled into the quad.
    for coordinates in [
        [[-40.0, 40.0], [40.0, 40.0], [40.0, -40.0], [10.0, 10.0]],
        [[-40.0, 40.0], [40.0, 40.0], [-10.0, 10.0], [-40.0, -40.0]],
    ] {
        let (image, corners) = render(coordinates);
        let (mut drawn, mut outside) = (0, 0);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let alpha = image.data[((y * SIDE + x) * 4 + 3) as usize];
                let center = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                // Clear of the outline by two pixels, past its antialiasing.
                let near = [(-2.0, -2.0), (2.0, -2.0), (-2.0, 2.0), (2.0, 2.0), (0.0, 0.0)]
                    .iter()
                    .any(|(dx, dy)| inside(&corners, (center.0 + dx, center.1 + dy)));
                if alpha > 0 {
                    drawn += 1;
                    if !near {
                        outside += 1;
                    }
                }
            }
        }
        assert!(drawn > 1000, "{coordinates:?}: {drawn} pixels drawn");
        assert_eq!(0, outside, "{coordinates:?}: pixels drawn outside the image's outline");
    }
}
