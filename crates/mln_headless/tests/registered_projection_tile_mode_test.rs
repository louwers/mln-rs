// A map in tile mode draws in its style's registered projection, as in the
// other modes: at the center of one of the projection's tiles, at the tile's
// zoom, that tile fills the image.

use std::sync::Arc;

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_engine::storage::{Kind, Resource, Response};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;
use parking_lot::Mutex;

const SIDE: u32 = 512;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A style of the raster source `tiles` in the identity projection, centered
/// on 45° E, 45° N at zoom 1: the center of the identity's tile 1/1/0, which
/// spans 0 to 90 E and 0 to 90 N.
const STYLE: &str = r#"{
    "version": 8,
    "center": [45, 45],
    "zoom": 1,
    "projection": {"type": "identity"},
    "sources": {"tiles": {"type": "raster", "tiles": ["tiles/{z}/{x}/{y}.png"], "tileSize": 512}},
    "layers": [{"id": "tiles", "type": "raster", "source": "tiles", "paint": {"raster-fade-duration": 0}}]
}"#;

/// A PNG of the opaque `color`.
fn png(color: [u8; 4]) -> Arc<Vec<u8>> {
    let mut image = PremultipliedImage::new(Size::new(8, 8));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        *pixel = color;
    }
    Arc::new(mln_engine::image::encode_png(&image))
}

#[test]
fn tile_mode_draws_a_tile_of_the_registered_projection() {
    let (queue, pool) = (Queue::new(), Pool::new(4));
    let size = Size::new(SIDE, SIDE);
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    // Tile 1/1/0 is red, and any other tile blue.
    let (red, blue) = (png(RED), png(BLUE));
    let requested = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::clone(&requested);
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        asked.lock().push(resource.url.clone());
        let mut response = Response::new();
        response.data = Some(Arc::clone(if resource.url == "tiles/1/1/0.png" { &red } else { &blue }));
        Some(response)
    });
    let options = MapOptions { map_mode: MapMode::Tile, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, mln_builtins::registry(), &pool).unwrap();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
    frontend.attach(&mut map);
    map.load_style_json(STYLE);
    let image = frontend.render(&mut map).expect("the tile").image;

    assert_eq!(vec!["tiles/1/1/0.png".to_owned()], *requested.lock());
    let not_red = image.data.as_chunks::<4>().0.iter().filter(|&&pixel| pixel != RED).count();
    assert_eq!(0, not_red, "pixels that aren't tile 1/1/0's");
}
