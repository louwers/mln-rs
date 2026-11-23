// Frontends on one device, as maplibre-native-ffi's offscreen device shares
// it: each draws its own map, in turn.

use mln_headless::HeadlessFrontend;
use mln_map_types::MapMode;
use mln_runtime::map::{MapCore, MapCoreOptions};
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;

/// What a frontend on `device` draws of a map whose background is `color`.
fn render_on(device: &mln_runtime::Device, color: &str) -> Vec<u8> {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::with_device(device.clone(), size, 1.0, Default::default(), &run_loop, &pool)
        .expect("a headless frontend");
    let mut map = MapCore::with_resource_options(
        MapCoreOptions { map_mode: MapMode::Static, size, ..Default::default() },
        &mln_storage::ResourceOptions::default(),
        crate::support::registry(),
        &pool,
    )
    .unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&format!(
        r#"{{"version": 8, "sources": {{}},
            "layers": [{{"id": "background", "type": "background", "paint": {{"background-color": "{color}"}}}}]}}"#
    ));
    frontend.render(&mut map).expect("render").image.data
}

#[test]
fn frontends_on_one_device_render_in_turn() {
    let device = mln_headless::harness_device().expect("device");
    for (color, pixel) in [("red", [255, 0, 0, 255]), ("blue", [0, 0, 255, 255]), ("red", [255, 0, 0, 255])] {
        let image = render_on(&device, color);
        assert!(image.as_chunks::<4>().0.iter().all(|px| *px == pixel), "{color}: {:?}", &image[..4]);
    }
}
