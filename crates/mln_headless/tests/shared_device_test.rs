// Frontends made with one device, as maplibre-native-ffi's offscreen device
// shares it: each draws its own map, in turn.

use std::sync::Arc;

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_headless::HeadlessFrontend;
use mln_style::size::Size;

/// What a frontend on `device` draws of a map whose background is `color`.
fn render_on(device: &mln_runtime::Device, color: &str) -> Vec<u8> {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::with_device(device.clone(), size, 1.0, Arc::default(), &run_loop, &pool)
        .expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&format!(
        r#"{{"version": 8, "sources": {{}},
            "layers": [{{"id": "background", "type": "background", "paint": {{"background-color": "{color}"}}}}]}}"#
    ));
    frontend.render(&mut map).expect("render").image.data
}

#[test]
fn frontends_made_with_one_device_each_draw_their_own_map() {
    let device = mln_headless::harness_device().expect("device");
    for (color, pixel) in [("red", [255, 0, 0, 255]), ("blue", [0, 0, 255, 255]), ("red", [255, 0, 0, 255])] {
        let image = render_on(&device, color);
        assert!(image.as_chunks::<4>().0.iter().all(|px| *px == pixel), "{color}: {:?}", &image[..4]);
    }
}
