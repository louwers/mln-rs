// Smoke tests for the HeadlessFrontend + Map still-image path (the C++
// tree tests this through test/api/*.test.cpp and the render tests), and of
// the frames the frontend draws when asked.

use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapMode, MapOptions};
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
    map.drain_events().iter().filter(|event| matches!(event, mln_engine::map::MapEvent::RenderFrameFinished(_))).count()
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
    use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_engine::storage::{Kind, Resource, Response};

    // A DEM that rises to the east, in the Terrarium encoding.
    let mut dem = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in dem.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (index % 256) as u8;
        pixel.copy_from_slice(&[128, x, 0, 255]);
    }
    let dem = Arc::new(mln_engine::image::encode_png(&dem));
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
    use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_engine::storage::{Kind, Resource, Response};

    // A DEM that rises to the east, in the Terrarium encoding.
    let mut dem = PremultipliedImage::new(Size::new(256, 256));
    for (index, pixel) in dem.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[128, (index % 256) as u8, 0, 255]);
    }
    let dem = Arc::new(mln_engine::image::encode_png(&dem));
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

/// One DEM tile of 1000 m everywhere, in the Mapbox encoding:
/// (1 · 65536 + 173 · 256 + 176) · 0.1 − 10000. With `found` false, the
/// source has no tile.
fn dem_source(found: bool) -> Arc<mln_engine::storage::stub_file_source::StubFileSource> {
    use mln_engine::storage::response::{Reason, ResourceError};
    use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_engine::storage::{Kind, Resource, Response};

    let mut dem = PremultipliedImage::new(Size::new(4, 4));
    for pixel in dem.data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[1, 173, 176, 255]);
    }
    let dem = Arc::new(mln_engine::image::encode_png(&dem));
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    file_source.set_response(Kind::Tile, move |_: &Resource| {
        let mut response = Response::new();
        if found {
            response.data = Some(Arc::clone(&dem));
        } else {
            response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "no such tile")));
        }
        Some(response)
    });
    file_source
}

/// Seen from zoom 1, one terrain tile covers the world, and reads the one
/// DEM tile, at zoom 0.
const TERRAIN_STYLE: &str = r#"{
    "version": 8,
    "zoom": 1,
    "terrain": {"source": "dem"},
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "maxzoom": 0}},
    "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]
}"#;

/// The center's altitude and the floor of the view's planes.
fn ground(map: &Map) -> (f64, Option<f64>) {
    let view = map.transform_state().view();
    (view.center_altitude(), view.terrain_min_elevation())
}

/// A still image of a terrain draws one frame, which shows the camera
/// standing on the ground: the renderer draws no frame of an update whose
/// camera hasn't stood on the ground it draws.
#[test]
fn a_still_image_over_terrain_draws_one_frame_of_the_camera_on_the_ground() {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(dem_source(true), options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(TERRAIN_STYLE);
    let result = frontend.render(&mut map).expect("the still image");
    assert_eq!(1, result.stats.num_frames);
    // The frame is of the map's last update, whose camera stands on the
    // ground.
    let latest = map.update_reader().latest().expect("an update");
    assert_eq!(latest.generation, frontend.drawn);
    let altitude = latest.parameters.view.center_altitude();
    assert!((altitude - 1000.0).abs() < 1e-6, "{altitude}");
    assert_eq!(pixel(&result.image, 32, 32), [255, 0, 0, 255]);

    // A renderer that lets go of its data tells the map at once that the
    // ground it drew is gone.
    frontend.clear_data();
    map.process_events();
    assert_eq!((0.0, None), ground(&map));
}

/// A still image over a terrain whose source has no DEM tile ends, with the
/// camera on flat ground.
#[test]
fn a_still_image_over_terrain_without_its_dem_tile_ends() {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(dem_source(false), options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(TERRAIN_STYLE);
    let result = frontend.render(&mut map).expect("the still image");
    assert_eq!(1, result.stats.num_frames);
    assert_eq!((0.0, Some(0.0)), ground(&map));
    assert_eq!(pixel(&result.image, 32, 32), [255, 0, 0, 255]);
}

/// A still image whose style loses its terrain before the image completes
/// ends on the flat map.
#[test]
fn a_still_image_whose_terrain_goes_away_ends_flat() {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(dem_source(true), options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(TERRAIN_STYLE);
    map.process_events();
    map.render_still().unwrap();
    // The first update finds the terrain, whose ground its camera hasn't
    // stood on: it draws no frame.
    assert!(frontend.render_update());
    assert_eq!(0, frontend.rendering_stats().num_frames);
    map.edit_style(|style| style.apply(mln_style::Edit::SetTerrain(None))).unwrap();
    let ended = (0..10_000).find_map(|_| {
        map.process_events();
        let ended = map.take_still_image();
        if ended.is_none() && !frontend.render_update() && !run_loop.run_once(&mut ()) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        ended
    });
    assert_eq!(Some(Ok(())), ended);
    assert_eq!(1, frontend.rendering_stats().num_frames);
    assert_eq!((0.0, None), ground(&map));
}

/// Asks for a still image and draws the map's updates until it ends, or
/// gives up after a while: `None` for a still that never ends.
fn try_still(map: &mut Map, frontend: &mut HeadlessFrontend, run_loop: &Queue<()>) -> Option<Result<(), Error>> {
    map.process_events();
    map.render_still().unwrap();
    (0..2_000).find_map(|_| {
        map.process_events();
        let ended = map.take_still_image();
        if ended.is_none() && !frontend.render_update() && !run_loop.run_once(&mut ()) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        ended
    })
}

/// A still image over terrain that the style dropped and then set again
/// as it was, between two images, draws: the map let go of the ground with
/// the terrain, and the renderer, whose ground is the same as before, tells
/// it of that ground again.
#[test]
fn a_still_image_over_terrain_removed_and_restored_draws() {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(dem_source(true), options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(TERRAIN_STYLE);
    assert_eq!(Some(Ok(())), try_still(&mut map, &mut frontend, &run_loop));
    assert_eq!(1, frontend.rendering_stats().num_frames);

    let terrain = map.style().get_terrain().map(|terrain| (**terrain).clone());
    map.edit_style(|style| style.apply(mln_style::Edit::SetTerrain(None))).unwrap();
    map.edit_style(|style| style.apply(mln_style::Edit::SetTerrain(terrain))).unwrap();
    assert_eq!(Some(Ok(())), try_still(&mut map, &mut frontend, &run_loop), "the second still never ended");
    assert_eq!(2, frontend.rendering_stats().num_frames);
    let altitude = map.update_reader().latest().expect("an update").parameters.view.center_altitude();
    assert!((altitude - 1000.0).abs() < 1e-6, "{altitude}");
}

/// The requests for tiles a file source answered: for all of them, and for
/// the far ones of [`far_dem`].
#[derive(Clone, Default)]
struct Requests {
    all: Arc<std::sync::atomic::AtomicUsize>,
    far: Arc<std::sync::atomic::AtomicUsize>,
}

impl Requests {
    fn all(&self) -> usize {
        self.all.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn far(&self) -> usize {
        self.far.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Seen from zoom 10.5 at a pitch of 60°, ground that is flat but for the
/// DEM tiles far from the camera, which stand high ([`far_dem`]).
const FAR_DEM_STYLE: &str = r#"{
    "version": 8,
    "center": [10, 45],
    "zoom": 10.5,
    "pitch": 60,
    "terrain": {"source": "dem", "exaggeration": 10},
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"], "encoding": "terrarium", "maxzoom": 14}},
    "layers": [{"id": "background", "type": "background", "paint": {"background-color": "red"}}]
}"#;

/// The DEM of [`FAR_DEM_STYLE`], and the requests for its tiles. Its tiles
/// of zoom 6 or more whose south edge lies north of the center's row of
/// terrain tiles stand high; terrain tiles read only them there. Once one
/// loads, its height range culls the terrain tiles that read it, so that
/// the terrain no longer reads it.
fn far_dem() -> (Arc<mln_engine::storage::stub_file_source::StubFileSource>, Requests) {
    use std::sync::atomic::Ordering;

    use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
    use mln_engine::storage::{Kind, Resource, Response};

    // DEM tiles of 16 pixels a side, each `height` m in Terrarium.
    let terrarium = |height: u16| {
        let mut dem = PremultipliedImage::new(Size::new(16, 16));
        let [hi, lo] = (height + 32768).to_be_bytes();
        for pixel in dem.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[hi, lo, 0, 255]);
        }
        Arc::new(mln_engine::image::encode_png(&dem))
    };
    let (low, high) = (terrarium(0), terrarium(20_000));
    let center_y = mln_util::projection::Projection::project(
        &mln_style::geo::LatLng::new(45.0, 10.0).unwrap(),
        1.0 / mln_style::constants::TILE_SIZE_D,
    )
    .y;
    let requests = Requests::default();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Synchronous));
    let counts = requests.clone();
    file_source.set_response(Kind::Tile, move |resource: &Resource| {
        let tile = resource.tile_data.as_ref().expect("a tile");
        let south = f64::from(tile.y + 1) / f64::from(1u32 << tile.z);
        let far = tile.z > 5 && south < center_y - 0.0003;
        counts.all.fetch_add(1, Ordering::Relaxed);
        if far {
            counts.far.fetch_add(1, Ordering::Relaxed);
        }
        let mut response = Response::new();
        response.data = Some(Arc::clone(if far { &high } else { &low }));
        Some(response)
    });
    (file_source, requests)
}

/// A still image draws, in one frame, over far DEM tiles that the terrain
/// lets go of once they load ([`far_dem`]), under a camera that stays put,
/// without the tile cache to keep them. The terrain remembers their height
/// ranges, so the next image of the same camera doesn't load them again.
#[test]
fn a_still_image_over_far_dem_tiles_that_flip_draws_in_one_frame() {
    let (file_source, requests) = far_dem();
    let size = Size::new(1024, 1024);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    frontend.renderer().set_tile_cache_enabled(false);
    let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(FAR_DEM_STYLE);
    assert_eq!(Some(Ok(())), try_still(&mut map, &mut frontend, &run_loop), "the still never ended");
    assert_eq!(1, frontend.rendering_stats().num_frames);
    assert_eq!((0.0, Some(0.0)), ground(&map));
    let far = requests.far();
    assert!(far > 0, "the far DEM tiles load");

    assert_eq!(Some(Ok(())), try_still(&mut map, &mut frontend, &run_loop), "the second still never ended");
    assert_eq!(2, frontend.rendering_stats().num_frames);
    assert_eq!(far, requests.far(), "the far DEM tiles load again");
}

/// A continuous map over far DEM tiles that the terrain lets go of once
/// they load ([`far_dem`]), without the tile cache to keep them, goes idle
/// and stays so: the terrain remembers their height ranges, so the cover
/// doesn't read them again, and no frame follows.
#[test]
fn a_continuous_map_over_far_dem_tiles_that_flip_settles() {
    let (file_source, requests) = far_dem();
    let size = Size::new(1024, 1024);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    frontend.renderer().set_tile_cache_enabled(false);
    let options = MapOptions { size, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(FAR_DEM_STYLE);
    // Draws the map's updates for at most `iterations` turns, and returns
    // whether the map went idle.
    let run = |map: &mut Map, frontend: &mut HeadlessFrontend, iterations: usize| {
        (0..iterations).any(|_| {
            map.process_events();
            if map.drain_events().iter().any(|event| matches!(event, mln_engine::map::MapEvent::Idle)) {
                return true;
            }
            if !frontend.render_update() && !run_loop.run_once(&mut ()) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            false
        })
    };
    assert!(run(&mut map, &mut frontend, 2_000), "the map never went idle");
    assert!(requests.far() > 0, "the far DEM tiles load");

    let (frames, all) = (frontend.rendering_stats().num_frames, requests.all());
    assert!(!run(&mut map, &mut frontend, 200), "the map went idle again");
    assert_eq!(frames, frontend.rendering_stats().num_frames, "frames follow the idle map's");
    assert_eq!(all, requests.all(), "DEM tiles load after the map went idle");
}

/// A continuous map over terrain goes idle only once a complete frame
/// showed the camera standing on the newest ground: the frame of an update
/// from before the camera stood on it isn't complete.
#[test]
fn a_continuous_map_over_terrain_goes_idle_on_the_ground() {
    let size = Size::new(64, 64);
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let options = MapOptions { size, ..Default::default() };
    let mut map = Map::with_file_source(dem_source(true), options, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(TERRAIN_STYLE);
    let idle = (0..10_000).any(|_| {
        map.process_events();
        if map.drain_events().iter().any(|event| matches!(event, mln_engine::map::MapEvent::Idle)) {
            // The last frame drew the map's last update, whose camera stands
            // on the ground.
            let latest = map.update_reader().latest().expect("an update");
            assert_eq!(latest.generation, frontend.drawn);
            let altitude = latest.parameters.view.center_altitude();
            assert!((altitude - 1000.0).abs() < 1e-6, "idle at {altitude} m");
            return true;
        }
        if !frontend.render_update() && !run_loop.run_once(&mut ()) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        false
    });
    assert!(idle, "the map never went idle");

    // A frame of an update that carries no ground, as one from before the
    // terrain drew would, draws everything that it shows but isn't
    // complete, and leaves the map as it is.
    let latest = map.update_reader().latest().expect("an update");
    frontend.draw(&Arc::new(UpdateParameters { elevation: None, ..(*latest.parameters).clone() }));
    map.process_events();
    let events = map.drain_events();
    let modes: Vec<mln_engine::RenderMode> = events
        .iter()
        .filter_map(|event| match event {
            mln_engine::map::MapEvent::RenderFrameFinished(status) => Some(status.mode),
            _ => None,
        })
        .collect();
    assert_eq!(vec![mln_engine::RenderMode::Partial], modes);
    assert!(!events.iter().any(|event| matches!(event, mln_engine::map::MapEvent::Idle)));
}
