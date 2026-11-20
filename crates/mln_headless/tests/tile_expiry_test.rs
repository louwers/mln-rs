// A map that stays on one view reloads its tiles as their data expires, and
// shows the tiles the server has by then (#644).

use std::sync::Arc;
use std::time::{Duration, Instant};

use httpmock::MockServer;
use mln_map::MapMode;
use mln_storage::network_status::NetworkStatus;
use mln_storage::online_file_source::OnlineFileSource;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::support::*;

const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn png(color: [u8; 4]) -> Vec<u8> {
    let mut image = PremultipliedImage::new(Size::new(4, 4));
    for pixel in image.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&color);
    }
    mln_image::encode_png(&image)
}

/// Runs the map's loop until its centre pixel is `color`, for at most ten
/// seconds.
fn wait_for_center(test: &MapTest<OnlineFileSource>, color: [u8; 4]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        run_within(&test.run_loop, Duration::from_millis(100));
        let image = test.frontend().read_still_image();
        if !image.data.is_empty() && center_pixel(&image) == color {
            return;
        }
        assert!(Instant::now() < deadline, "the centre never turned {color:?}");
    }
}

#[test]
fn a_tile_reloads_when_its_data_expires() {
    let server = MockServer::start();
    let tiles = r"^/\d+/\d+/\d+\.png$";
    let mut first = server.mock(|when, then| {
        when.method("GET").path_matches(tiles);
        then.status(200).header("Cache-Control", "max-age=1").body(png(GREEN));
    });
    let file_source = Arc::new(OnlineFileSource::new(Arc::new(NetworkStatus::new())));
    let test = MapTest::create(TestLoop::new(), file_source, 1.0, MapMode::Continuous);
    test.load_json(&format!(
        r#"{{
  "version": 8,
  "center": [0, 0],
  "zoom": 0,
  "sources": {{ "tiles": {{ "type": "raster", "tileSize": 256, "tiles": ["{}"] }} }},
  "layers": [{{ "id": "tiles", "type": "raster", "source": "tiles", "paint": {{ "raster-fade-duration": 0 }} }}]
}}"#,
        server.url("/{z}/{x}/{y}.png")
    ));
    wait_for_center(&test, GREEN);

    // The server's tiles change; the map's expire a second after they came.
    first.delete();
    server.mock(|when, then| {
        when.method("GET").path_matches(tiles);
        then.status(200).header("Cache-Control", "max-age=3600").body(png(BLUE));
    });
    wait_for_center(&test, BLUE);
}
