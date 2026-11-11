// Smoke tests for the HeadlessFrontend + Map still-image path (the C++
// tree tests this through test/api/*.test.cpp and the render tests).

use mln_map::{MapMode, MapOptions, NullMapObserver};

use super::*;

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.size.width + x) * 4) as usize;
    [image.data[i], image.data[i + 1], image.data[i + 2], image.data[i + 3]]
}

fn render_style(style: &str, size: Size, pixel_ratio: f32) -> PremultipliedImage {
    let _run_loop = RunLoop::new();
    let mut frontend = HeadlessFrontend::new(size, pixel_ratio);
    let mut map = Map::new(
        &frontend,
        Rc::new(NullMapObserver),
        MapOptions::new().with_map_mode(MapMode::Static).with_size(size).with_pixel_ratio(pixel_ratio),
    )
    .unwrap();
    map.get_style_mut().load_json(style);
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
    let mut frontend = HeadlessFrontend::new(size, 1.0);
    let mut map = Map::new(&frontend, Rc::new(NullMapObserver), MapOptions::new().with_size(size)).unwrap();
    let error = frontend.render(&mut map).err().expect("error");
    assert_eq!(error, "Map is not in static or tile image render modes");
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
