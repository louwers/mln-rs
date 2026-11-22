// Smoke tests for the HeadlessFrontend + Map still-image path (the C++
// tree tests this through test/api/*.test.cpp and the render tests), and of
// the frames the frontend draws when asked.

use mln_map::{CameraOptions, MapMode, MapOptions};
use mln_util::pool::Pool;

use super::*;

fn pixel(image: &PremultipliedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.size.width + x) * 4) as usize;
    [image.data[i], image.data[i + 1], image.data[i + 2], image.data[i + 3]]
}

fn render_style(style: &str, size: Size, pixel_ratio: f32) -> PremultipliedImage {
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, pixel_ratio, &run_loop, &pool).expect("a headless frontend");
    let mut map =
        Map::new(MapOptions { map_mode: MapMode::Static, size, pixel_ratio, ..Default::default() }, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(style);
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
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let mut map = Map::new(MapOptions { size, ..Default::default() }, &pool).unwrap();
    frontend.attach(&mut map);
    let error = frontend.render(&mut map).err().expect("error");
    assert_eq!(error, Error::invalid_state("Map is not in static or tile image render modes"));
}

/// A style of one background layer of `color`.
fn background_style(color: &str) -> String {
    format!(
        r#"{{"version": 8, "sources": {{}}, "layers": [{{"id": "background", "type": "background", "paint": {{"background-color": "{color}"}}}}]}}"#
    )
}

/// How many frames the map's renderer finished since the last call.
fn frames(map: &mut Map) -> usize {
    map.drain_events().iter().filter(|event| matches!(event, mln_map::MapEvent::RenderFrameFinished(_))).count()
}

#[test]
fn render_update_draws_each_update_of_the_attached_map_once() {
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let mut map = Map::new(MapOptions { size, ..Default::default() }, &pool).unwrap();
    map.load_style_json(&background_style("red"));
    map.drain_events();
    // The map published before the frontend attached: the frontend draws the
    // latest update.
    frontend.attach(&mut map);
    assert!(frontend.render_update());
    // Nothing new until the map publishes again.
    assert!(!frontend.render_update());
    assert_eq!(frames(&mut map), 1);
    assert_eq!(pixel(&frontend.read_still_image(), 0, 0), [255, 0, 0, 255]);

    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    assert!(frontend.render_update());
    assert_eq!(frames(&mut map), 1);

    // render_frame draws the latest update again.
    assert!(frontend.render_frame());
    assert_eq!(frames(&mut map), 1);
}

#[test]
fn render_update_drops_the_renderer_once_the_map_is_gone() {
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let mut map = Map::new(MapOptions { size, ..Default::default() }, &pool).unwrap();
    map.load_style_json(&background_style("red"));
    map.process_events();
    frontend.attach(&mut map);
    assert!(frontend.render_update());
    drop(map);
    assert!(!frontend.render_update());
    assert!(frontend.target.renderer().is_none());
    assert!(!frontend.render_frame());
}

#[test]
fn attaching_another_map_renders_that_map() {
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let size = Size::new(16, 16);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let mut first = Map::new(MapOptions { size, ..Default::default() }, &pool).unwrap();
    let mut second = Map::new(MapOptions { size, ..Default::default() }, &pool).unwrap();
    first.load_style_json(&background_style("red"));
    second.load_style_json(&background_style("blue"));
    first.drain_events();
    second.drain_events();
    frontend.attach(&mut first);
    frontend.attach(&mut second);
    assert!(frontend.render_update());
    first.drain_events();
    assert_eq!(frames(&mut second), 1);
    assert_eq!(pixel(&frontend.read_still_image(), 0, 0), [0, 0, 255, 255]);

    // The first map's updates no longer reach the frontend.
    first.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    frontend.render_update();
    assert_eq!(frames(&mut first), 0);
    assert_eq!(pixel(&frontend.read_still_image(), 0, 0), [0, 0, 255, 255]);
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
    assert!((i32::from(p[0]) - 128).abs() <= 1, "{p:?}");
    assert_eq!(p[1], 0, "{p:?}");
    assert!((i32::from(p[2]) - 128).abs() <= 1, "{p:?}");
    assert_eq!(p[3], 255, "{p:?}");
}

/// A hillshade lit from the viewport, as its lights are by default, turns
/// them with the map: after a rotation it draws as a map that starts
/// rotated does.
#[test]
fn a_viewport_lit_hillshade_turns_its_lights_with_the_map() {
    use mln_storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_storage::{Kind, Resource, Response};

    // A DEM that rises to the east, in the Terrarium encoding.
    let mut dem = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in dem.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (index % 256) as u8;
        pixel.copy_from_slice(&[128, x, 0, 255]);
    }
    let dem = Arc::new(mln_image::encode_png(&dem));
    let style = r#"{
        "version": 8,
        "zoom": 2,
        "sources": {"dem": {"type": "raster-dem", "encoding": "terrarium", "tiles": ["dem/{z}/{x}/{y}.png"], "maxzoom": 2}},
        "layers": [{"id": "hillshade", "type": "hillshade", "source": "dem", "paint": {"hillshade-exaggeration": 1}}]
    }"#;
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let render = |bearings: &[f64]| -> PremultipliedImage {
        let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
        let dem = Arc::clone(&dem);
        file_source.set_response(Kind::Tile, move |_: &Resource| {
            let mut response = Response::new();
            response.data = Some(Arc::clone(&dem));
            Some(response)
        });
        let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
        let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
        let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
        frontend.attach(&mut map);
        map.load_style_json(style);
        let mut image = PremultipliedImage::new(Size::default());
        for &bearing in bearings {
            map.jump_to(&CameraOptions { bearing: Some(bearing), ..Default::default() });
            image = frontend.render(&mut map).expect("the still image").image;
        }
        image
    };
    let turned = render(&[0.0, 90.0]);
    assert_ne!(render(&[0.0]), turned, "the hillshade shows its lights' direction");
    assert_eq!(render(&[90.0]), turned);
}

/// A hillshade prepares each DEM tile's slopes once: a second still of the
/// same view draws the same image without the prepare draws.
#[test]
fn a_hillshade_prepares_its_slopes_once() {
    use mln_storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_storage::{Kind, Resource, Response};

    // A DEM that rises to the east, in the Terrarium encoding.
    let mut dem = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in dem.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[128, (index % 256) as u8, 0, 255]);
    }
    let dem = Arc::new(mln_image::encode_png(&dem));
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    file_source.set_response(Kind::Tile, move |_: &Resource| {
        let mut response = Response::new();
        response.data = Some(Arc::clone(&dem));
        Some(response)
    });
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(
        r#"{
        "version": 8,
        "zoom": 2,
        "sources": {"dem": {"type": "raster-dem", "encoding": "terrarium", "tiles": ["dem/{z}/{x}/{y}.png"], "maxzoom": 2}},
        "layers": [{"id": "hillshade", "type": "hillshade", "source": "dem", "paint": {"hillshade-exaggeration": 1}}]
    }"#,
    );

    let first = frontend.render(&mut map).expect("the first still");
    let second = frontend.render(&mut map).expect("the second still");
    let third = frontend.render(&mut map).expect("the third still");
    assert_eq!(first.image, second.image);
    assert_eq!(second.image, third.image);
    assert!(
        second.stats.num_draw_calls < first.stats.num_draw_calls,
        "{} draws, then {}",
        first.stats.num_draw_calls,
        second.stats.num_draw_calls
    );
    assert_eq!(second.stats.num_draw_calls, third.stats.num_draw_calls);
}
