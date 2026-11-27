// gtests-core: test/renderer/symbol_rendering.test.cpp (maplibre-native
// #4628): symbol drawables that placement hides completely draw nothing,
// and draw again once placement shows them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::exec::pool::Pool;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::storage::Kind;
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::conversion::Convertible;
use mln_style::geo::LatLng;
use mln_style::layer::Layer;
use mln_style::size::Size;
use mln_style::types::VisibilityType;

use crate::support::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SymbolKind {
    Icon,
    SdfIcon,
    Text,
    TextAndIcon,
}

/// A style of `count` points `spacing` degrees of longitude apart from
/// (5, 5), each with its own sort key.
fn colliding_symbols_style(count: usize, spacing: f64, kind: SymbolKind) -> String {
    let features: Vec<String> = (0..count)
        .map(|i| {
            format!(
                r#"{{"type":"Feature","properties":{{"rank":{i}}},"geometry":{{"type":"Point","coordinates":[{},5]}}}}"#,
                5.0 + i as f64 * spacing
            )
        })
        .collect();
    let mut layout = vec![r#""symbol-sort-key":["get","rank"]"#.to_owned()];
    if kind != SymbolKind::Text {
        layout.push(r#""icon-image":"marker""#.to_owned());
    }
    if matches!(kind, SymbolKind::Text | SymbolKind::TextAndIcon) {
        layout.push(r#""text-field":"Label","text-font":["Open Sans Regular"],"text-size":16"#.to_owned());
    }
    format!(
        r#"{{
            "version": 8,
            "glyphs": "glyphs/{{fontstack}}/{{range}}.pbf",
            "sources": {{
                "points": {{
                    "type": "geojson",
                    "maxzoom": 0,
                    "data": {{"type": "FeatureCollection", "features": [{}]}}
                }}
            }},
            "layers": [{{
                "id": "symbols",
                "type": "symbol",
                "source": "points",
                "layout": {{{}}},
                "paint": {{"text-halo-color": "white", "text-halo-width": 1}}
            }}]
        }}"#,
        features.join(","),
        layout.join(",")
    )
}

fn marker(sdf: bool) -> Arc<mln_style::image::Impl> {
    let image = PremultipliedImage::from_vec(Size::new(16, 16), vec![255; 16 * 16 * 4]);
    Arc::new(
        mln_style::image::Impl::new("marker".to_owned(), image, 1.0, sdf, Vec::new(), Vec::new(), None, None, None)
            .expect("style::Image"),
    )
}

fn lat_lng(latitude: f64, longitude: f64) -> LatLng {
    LatLng::new(latitude, longitude).expect("a coordinate")
}

struct SymbolRenderingTest {
    _events: Observing,
    run_loop: TestLoop,
    frontend: Rc<RefCell<HeadlessFrontend>>,
    observer: Rc<StubMapObserver>,
    map: Rc<RefCell<Map>>,
}

impl SymbolRenderingTest {
    /// A test whose frontend draws each update of the map from the run loop.
    fn new(count: usize, spacing: f64, kind: SymbolKind, mode: MapMode) -> Self {
        Self::with_frontend(true, count, spacing, kind, mode)
    }

    /// A test whose frontend loads resources asynchronously, but renders a
    /// frame only when the test asks ([`render_at`](Self::render_at)), at
    /// the time it chooses.
    fn timed(count: usize, spacing: f64, kind: SymbolKind, mode: MapMode) -> Self {
        Self::with_frontend(false, count, spacing, kind, mode)
    }

    fn with_frontend(live: bool, count: usize, spacing: f64, kind: SymbolKind, mode: MapMode) -> Self {
        let run_loop = TestLoop::new();
        let pool = Pool::new(4);
        let mut frontend =
            HeadlessFrontend::new(Size::new(256, 256), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
        let file_source = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
        file_source.set_response(Kind::Glyphs, |_| Some(file_response("test/fixtures/resources/glyphs.pbf")));
        // A style URL never answers: the map keeps the style it has.
        file_source.set_response(Kind::Style, |_| None);
        let observer = Rc::new(StubMapObserver::default());
        let mut map = Map::with_file_source(
            file_source,
            MapOptions { map_mode: mode, size: Size::new(256, 256), ..Default::default() },
            mln_builtins::registry(),
            &pool,
        )
        .unwrap();
        frontend.attach(&mut map);
        map.load_style_json(&colliding_symbols_style(count, spacing, kind));
        map.edit_style(|style| style.apply(Edit::AddImage(marker(kind == SymbolKind::SdfIcon)))).unwrap();
        map.jump_to(&CameraOptions { center: Some(lat_lng(5.0, 5.0)), zoom: Some(4.0), ..Default::default() });
        let map = Rc::new(RefCell::new(map));
        let frontend = Rc::new(RefCell::new(frontend));
        let events = observer.observe(&run_loop, &map, live.then_some(&frontend));
        SymbolRenderingTest { _events: events, run_loop, frontend, observer, map }
    }

    fn render(&self) -> RenderResult {
        let result = self.frontend.borrow_mut().render(&mut self.map.borrow_mut()).expect("render");
        self.observer.dispatch_events(&self.map);
        result
    }

    /// Draws the map's latest update as at `time`.
    fn render_at(&self, time: Instant) -> RenderResult {
        let update = self.map.borrow().update_reader().latest().expect("update parameters");
        let mut parameters = (*update.parameters).clone();
        parameters.time_point = Some(time);
        let mut frontend = self.frontend.borrow_mut();
        frontend.draw(&Arc::new(parameters));
        RenderResult { image: frontend.read_still_image(), stats: frontend.rendering_stats() }
    }

    fn jump_to(&self, camera: CameraOptions) {
        self.map.borrow_mut().jump_to(&camera);
    }
}

// gtest: SymbolRendering.CollisionRejectedSymbolsDoNotDraw
#[test]
fn collision_rejected_symbols_do_not_draw() {
    for kind in [SymbolKind::Icon, SymbolKind::SdfIcon, SymbolKind::Text, SymbolKind::TextAndIcon] {
        let reference = SymbolRenderingTest::new(1, 0.0, kind, MapMode::Static).render();

        let test = SymbolRenderingTest::new(256, 0.0, kind, MapMode::Static);
        let result = test.render();
        assert!(result.stats.num_draw_calls > 0, "{kind:?}");
        assert_eq!(result.stats.num_draw_calls, reference.stats.num_draw_calls, "{kind:?}");
        assert!(result.image == reference.image, "{kind:?}");

        let next = test.render();
        assert_eq!(next.stats.num_draw_calls, reference.stats.num_draw_calls, "{kind:?}");
        assert!(result.image == next.image, "{kind:?}");
    }
}

// gtest: SymbolRendering.CollisionVisibilityUpdatesExistingDrawables
#[test]
fn collision_visibility_updates_existing_drawables() {
    let test = SymbolRenderingTest::new(2, 0.5, SymbolKind::Icon, MapMode::Static);
    test.jump_to(CameraOptions { center: Some(lat_lng(5.0, 5.25)), zoom: Some(4.1), ..Default::default() });
    let overlapping = test.render();
    assert_eq!(overlapping.stats.num_draw_calls, 1);

    test.jump_to(CameraOptions { zoom: Some(4.9), ..Default::default() });
    let separated = test.render();
    assert_eq!(separated.stats.num_draw_calls, 2);

    test.jump_to(CameraOptions { zoom: Some(4.1), ..Default::default() });
    let overlapping_again = test.render();
    assert_eq!(overlapping_again.stats.num_draw_calls, 1);
    assert_eq!(overlapping.image, overlapping_again.image);
}

/// Runs the loop until `done` or ten seconds pass, so that a test fails
/// rather than hangs when hidden drawables never stop drawing or never
/// draw again.
fn run_until(run_loop: &TestLoop, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() && Instant::now() < deadline {
        if !run_loop.run_once() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

// gtest: SymbolRendering.ContinuousCollisionVisibilityFadesAndReappears
#[test]
fn continuous_collision_visibility_fades_and_reappears() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Phase {
        Initial,
        Colliding,
        Reappearing,
        Complete,
    }

    let test = SymbolRenderingTest::new(2, 0.5, SymbolKind::Icon, MapMode::Continuous);
    test.jump_to(CameraOptions { center: Some(lat_lng(5.0, 5.25)), zoom: Some(4.9), ..Default::default() });

    let phase = Rc::new(Cell::new(Phase::Initial));
    let failures = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let phase = Rc::clone(&phase);
        let failures = Rc::clone(&failures);
        let frontend = Rc::clone(&test.frontend);
        let map = Rc::downgrade(&test.map);
        let saw_fade_out = Cell::new(false);
        let initial_image = RefCell::new(None::<PremultipliedImage>);
        test.observer.did_finish_rendering_frame_callback.set(move |status| {
            if status.mode != RenderMode::Full {
                return;
            }
            let Some(map) = map.upgrade() else {
                return;
            };
            let draw_calls = status.rendering_stats.num_draw_calls;
            match phase.get() {
                Phase::Initial => {
                    if status.needs_repaint {
                        return;
                    }
                    if draw_calls != 2 {
                        failures.borrow_mut().push(format!("initial: {draw_calls} draw calls"));
                    }
                    *initial_image.borrow_mut() = Some(frontend.borrow_mut().read_still_image());
                    phase.set(Phase::Colliding);
                    map.borrow_mut().jump_to(&CameraOptions { zoom: Some(4.1), ..Default::default() });
                }
                Phase::Colliding => {
                    // A committed collision keeps the fading symbol's
                    // drawable enabled.
                    if status.placement_changed && status.needs_repaint && draw_calls == 2 {
                        saw_fade_out.set(true);
                    }
                    if draw_calls != 1 {
                        return;
                    }
                    if !saw_fade_out.get() {
                        failures.borrow_mut().push("the colliding symbol didn't fade out".to_owned());
                    }
                    phase.set(Phase::Reappearing);
                    map.borrow_mut().jump_to(&CameraOptions { zoom: Some(4.9), ..Default::default() });
                }
                Phase::Reappearing => {
                    if status.needs_repaint {
                        return;
                    }
                    if draw_calls != 2 {
                        failures.borrow_mut().push(format!("reappeared: {draw_calls} draw calls"));
                    }
                    if initial_image.borrow().as_ref() != Some(&frontend.borrow_mut().read_still_image()) {
                        failures.borrow_mut().push("the reappeared frame differs from the initial one".to_owned());
                    }
                    phase.set(Phase::Complete);
                }
                Phase::Complete => {}
            }
        });
    }

    run_until(&test.run_loop, || phase.get() == Phase::Complete);
    test.observer.did_finish_rendering_frame_callback.clear();
    assert_eq!(phase.get(), Phase::Complete);
    assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
}

// gtest: SymbolRendering.CollisionFadeIntermediateFrames
#[test]
fn collision_fade_intermediate_frames() {
    let test = SymbolRenderingTest::timed(2, 0.5, SymbolKind::Icon, MapMode::Continuous);
    test.jump_to(CameraOptions { center: Some(lat_lng(5.0, 5.25)), zoom: Some(4.9), ..Default::default() });

    let fully_loaded = Rc::new(Cell::new(false));
    {
        let fully_loaded = Rc::clone(&fully_loaded);
        test.observer
            .did_finish_rendering_frame_callback
            .set(move |status| fully_loaded.set(status.mode == RenderMode::Full));
    }
    let start = Instant::now();
    let frame = |ms: u64| {
        test.run_loop.run_once();
        test.render_at(start + Duration::from_millis(ms))
    };

    // Wait only for resources; simulated time stays fixed while loading.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fully_loaded.get() && Instant::now() < deadline {
        frame(0);
    }
    assert!(fully_loaded.get());
    // Commit the initial placement, then let its fade finish before changing
    // collisions.
    frame(1000);
    let initial = frame(1400);
    assert_eq!(initial.stats.num_draw_calls, 2);

    let alpha_at_symbol = |image: &PremultipliedImage, longitude: f64| {
        let pixel = test.map.borrow().pixel_for_lat_lng(&lat_lng(5.0, longitude));
        let (x, y) = (pixel.x as u32, pixel.y as u32);
        image.data[(4 * (y * image.size.width + x) + 3) as usize]
    };
    assert_eq!(alpha_at_symbol(&initial.image, 5.5), 255);

    // Rotation makes the icon collision boxes overlap without zoom's fade
    // adjustment.
    test.jump_to(CameraOptions { bearing: Some(45.0), ..Default::default() });
    let fade_out_start = frame(2000);
    let fade_out_middle = frame(2150);
    let fade_out_end = frame(2400);
    assert_eq!(fade_out_start.stats.num_draw_calls, 2);
    assert_eq!(fade_out_middle.stats.num_draw_calls, 2);
    assert!(alpha_at_symbol(&fade_out_start.image, 5.5).abs_diff(255) <= 1);
    assert!(alpha_at_symbol(&fade_out_middle.image, 5.5).abs_diff(128) <= 2);
    assert_eq!(alpha_at_symbol(&fade_out_end.image, 5.5), 0);
    assert_eq!(alpha_at_symbol(&fade_out_middle.image, 5.0), 255);
    let hidden = frame(2600);
    assert_eq!(hidden.stats.num_draw_calls, 1);
    assert_eq!(hidden.image, fade_out_end.image);

    test.jump_to(CameraOptions { bearing: Some(0.0), ..Default::default() });
    let fade_in_start = frame(3000);
    let fade_in_middle = frame(3150);
    let fade_in_end = frame(3400);
    assert_eq!(fade_in_start.stats.num_draw_calls, 2);
    assert_eq!(fade_in_middle.stats.num_draw_calls, 2);
    assert_eq!(alpha_at_symbol(&fade_in_start.image, 5.5), 0);
    assert!(alpha_at_symbol(&fade_in_middle.image, 5.5).abs_diff(128) <= 2);
    assert_eq!(alpha_at_symbol(&fade_in_end.image, 5.5), 255);
    assert_eq!(alpha_at_symbol(&fade_in_middle.image, 5.0), 255);
    assert_eq!(fade_in_end.image, initial.image);
}

// gtest: SymbolRendering.FullyHiddenLayerCanReappear
#[test]
fn fully_hidden_layer_can_reappear() {
    let test = SymbolRenderingTest::new(256, 0.0, SymbolKind::Icon, MapMode::Static);
    let initial = test.render();

    let mut blocker = Layer::new(mln_builtins::registry().style(), "symbol", "blocker", "points").unwrap();
    let error = blocker.set_property("icon-image", &Convertible::from(&serde_json::json!("marker")), None);
    assert!(error.is_none(), "{error:?}");
    test.map.borrow_mut().edit_style(|style| style.apply(Edit::AddLayer { layer: blocker, before: None })).unwrap();
    let hidden = test.render();
    assert_eq!(hidden.stats.num_draw_calls, 1);
    assert_eq!(initial.image, hidden.image);

    test.map
        .borrow_mut()
        .edit_style(|style| {
            style.apply(Edit::SetLayerVisibility { layer: "blocker".to_owned(), visibility: VisibilityType::None })
        })
        .expect("the blocker layer");
    let visible = test.render();
    assert_eq!(visible.stats.num_draw_calls, 1);
    assert_eq!(initial.image, visible.image);
}

/// Port-only: while a new style URL loads, the tiles of the style before
/// keep drawing, and an image they draw that changes in place shows its new
/// pixels.
#[test]
fn shown_icons_take_image_updates_while_a_style_url_loads() {
    let test = SymbolRenderingTest::timed(1, 0.0, SymbolKind::Icon, MapMode::Continuous);
    let fully_loaded = Rc::new(Cell::new(false));
    {
        let fully_loaded = Rc::clone(&fully_loaded);
        test.observer
            .did_finish_rendering_frame_callback
            .set(move |status| fully_loaded.set(status.mode == RenderMode::Full));
    }
    let start = Instant::now();
    let frame = |ms: u64| {
        test.run_loop.run_once();
        test.render_at(start + Duration::from_millis(ms))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fully_loaded.get() && Instant::now() < deadline {
        frame(0);
    }
    assert!(fully_loaded.get());
    let pixel_at_symbol = |image: &PremultipliedImage| {
        let pixel = test.map.borrow().pixel_for_lat_lng(&lat_lng(5.0, 5.0));
        let offset = (4 * (pixel.y as u32 * image.size.width + pixel.x as u32)) as usize;
        <[u8; 4]>::try_from(&image.data[offset..offset + 4]).expect("a pixel")
    };
    assert_eq!(pixel_at_symbol(&frame(1000).image), [255; 4]);

    test.map.borrow_mut().load_style_url("https://example.com/next.json");
    frame(1100);
    let red = PremultipliedImage::from_vec(Size::new(16, 16), [255, 0, 0, 255].repeat(16 * 16));
    let red =
        mln_style::image::Impl::new("marker".to_owned(), red, 1.0, false, Vec::new(), Vec::new(), None, None, None)
            .expect("style::Image");
    test.map.borrow_mut().edit_style(|style| style.apply(Edit::AddImage(Arc::new(red)))).unwrap();
    frame(1200);
    assert_eq!(pixel_at_symbol(&frame(1300).image), [255, 0, 0, 255]);
}
