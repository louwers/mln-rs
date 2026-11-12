// Smoke tests for the HeadlessFrontend + Map still-image path (the C++
// tree tests this through test/api/*.test.cpp and the render tests).

use mln_map::{MapMode, MapOptions};

use super::*;

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.size.width + x) * 4) as usize;
    [image.data[i], image.data[i + 1], image.data[i + 2], image.data[i + 3]]
}

fn render_style(style: &str, size: Size, pixel_ratio: f32) -> PremultipliedImage {
    let _run_loop = RunLoop::new();
    let mut frontend = HeadlessFrontend::new(size, pixel_ratio).expect("a headless frontend");
    let mut map =
        Map::new(MapOptions::new().with_map_mode(MapMode::Static).with_size(size).with_pixel_ratio(pixel_ratio))
            .unwrap();
    frontend.attach(&map);
    map.style_mut().load_json(style);
    frontend.render(&mut map).expect("render").image
}

#[test]
fn render_still_returns_image_of_physical_size() {
    let image = render_style(r#"{"version": 8, "sources": {}, "layers": []}"#, Size::new(64, 32), 2.0);
    assert_eq!(image.size, Size::new(128, 64));
    // No layers: the clear color is the default (transparent) Color.
    assert_eq!(pixel(&image, 10, 10), [0, 0, 0, 0]);
}

#[test]
fn render_still_in_continuous_mode_is_an_error() {
    let _run_loop = RunLoop::new();
    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0).expect("a headless frontend");
    let mut map = Map::new(MapOptions::new().with_size(size)).unwrap();
    frontend.attach(&map);
    let error = frontend.render(&mut map).err().expect("error");
    assert_eq!(error, mln_util::Error::invalid_state("Map is not in static or tile image render modes"));
}

/// How many frames the map's renderer finished since the last call.
fn frames(map: &mut Map) -> usize {
    map.drain_events().iter().filter(|event| matches!(event, mln_map::MapEvent::RenderFrameFinished(_))).count()
}

#[test]
fn draws_each_update_of_the_attached_map_on_the_run_loop() {
    let run_loop = RunLoop::new();
    let size = Size::new(16, 16);
    let frontend = HeadlessFrontend::new(size, 1.0).expect("a headless frontend");
    let mut map = Map::new(MapOptions::new().with_size(size)).unwrap();
    map.style_mut().load_json(
        r#"{"version": 8, "sources": {}, "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]}"#,
    );
    map.drain_events();
    // The map published before the frontend attached: the frontend draws the
    // latest update at once.
    frontend.attach(&map);
    run_loop.run_once();
    assert_eq!(frames(&mut map), 1);

    map.jump_to(&CameraOptions::default().with_zoom(Some(2.0)));
    run_loop.run_once();
    assert_eq!(frames(&mut map), 1);
    assert_eq!(frontend.get_camera_options().zoom, Some(2.0));
}

#[test]
fn attaching_another_map_renders_that_map() {
    let run_loop = RunLoop::new();
    let size = Size::new(16, 16);
    let frontend = HeadlessFrontend::new(size, 1.0).expect("a headless frontend");
    let mut first = Map::new(MapOptions::new().with_size(size)).unwrap();
    let mut second = Map::new(MapOptions::new().with_size(size)).unwrap();
    first.jump_to(&CameraOptions::default().with_zoom(Some(1.0)));
    second.jump_to(&CameraOptions::default().with_zoom(Some(3.0)));
    frontend.attach(&first);
    assert_eq!(frontend.get_camera_options().zoom, Some(1.0));
    frontend.attach(&second);
    run_loop.run_once();
    first.drain_events();
    assert_eq!(frames(&mut second), 1);
    assert_eq!(frontend.get_camera_options().zoom, Some(3.0));

    // The first map's updates no longer reach the frontend.
    first.jump_to(&CameraOptions::default().with_zoom(Some(2.0)));
    run_loop.run_once();
    assert_eq!(frames(&mut first), 0);
    assert_eq!(frontend.get_camera_options().zoom, Some(3.0));
}

#[test]
fn solid_background_is_the_clear_color() {
    let image = render_style(
        r#"{"version": 8, "sources": {}, "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]}"#,
        Size::new(16, 16),
        1.0,
    );
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 15, 15), [255, 0, 0, 255]);
}

#[test]
fn second_background_layer_uses_the_drawable_path() {
    // Only the first layer can become the clear color; the second one is
    // drawn with drawables (as in background-opacity/overlay).
    let image = render_style(
        r#"{"version": 8, "sources": {}, "layers": [
            {"id": "a", "type": "background", "paint": {"background-color": "red"}},
            {"id": "b", "type": "background", "paint": {"background-color": "blue", "background-opacity": 0.5}}
        ]}"#,
        Size::new(16, 16),
        1.0,
    );
    let p = pixel(&image, 8, 8);
    // red * 0.5 + blue * 0.5 (premultiplied, alpha blended)
    assert!((p[0] as i32 - 128).abs() <= 1, "{p:?}");
    assert_eq!(p[1], 0, "{p:?}");
    assert!((p[2] as i32 - 128).abs() <= 1, "{p:?}");
    assert_eq!(p[3], 255, "{p:?}");
}
