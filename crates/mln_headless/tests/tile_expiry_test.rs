// A map that stays on one view shows each answer that its tiles' open
// requests get, as the file source revalidates their data when it expires
// (#644). When the online file source revalidates, and how it reads the
// servers' caching headers, are its own tests' rules.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_map::MapMode;
use mln_storage::{Kind, Response};
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::*;

const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn png(color: [u8; 4]) -> Response {
    let mut image = PremultipliedImage::new(Size::new(4, 4));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&color);
    }
    let mut response = Response::new();
    response.data = Some(Arc::new(mln_image::encode_png(&image)));
    response
}

/// Answers every tile request that `test`'s file source holds with
/// `response`, then runs the map until it is idle, and returns its centre
/// pixel.
fn answer_tiles(test: &MapTest<FakeFileSource>, response: &Response) -> [u8; 4] {
    let deadline = Instant::now() + Duration::from_secs(60);
    while test.file_source.respond_all(Kind::Tile, response) == 0 {
        assert!(Instant::now() < deadline, "no open tile request");
        run_within(&test.run_loop, Duration::from_millis(10));
    }
    {
        let run_loop = test.run_loop.clone();
        test.observer.did_become_idle_callback.set(move |()| run_loop.stop());
    }
    run_with_timeout(&test.run_loop, 60);
    test.observer.did_become_idle_callback.clear();
    center_pixel(&test.frontend().read_still_image())
}

#[test]
fn a_tile_shows_each_answer_to_its_open_request() {
    let test = MapTest::create(TestLoop::new(), Arc::new(FakeFileSource::new()), 1.0, MapMode::Continuous);
    test.load_json(
        r#"{
  "version": 8,
  "center": [0, 0],
  "zoom": 0,
  "sources": { "tiles": { "type": "raster", "tileSize": 256, "tiles": ["test://tiles/{z}/{x}/{y}.png"] } },
  "layers": [{ "id": "tiles", "type": "raster", "source": "tiles", "paint": { "raster-fade-duration": 0 } }]
}"#,
    );
    assert_eq!(GREEN, answer_tiles(&test, &png(GREEN)));
    let requests = test.file_source.loads();

    // The file source answers the same requests again, as it does when it
    // revalidates data that expired and the server has new data.
    assert_eq!(BLUE, answer_tiles(&test, &png(BLUE)));
    assert_eq!(requests, test.file_source.loads(), "no new request");
}
