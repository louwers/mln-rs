// gtests-core: test/renderer/symbol_rendering.test.cpp (maplibre-native
// #4628): symbol drawables that placement hides completely draw nothing,
// and draw again once placement shows them.

mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_actor::{AsyncTask, RunLoop};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::{BackendScope, ScopeType};
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_map::{CameraOptions, Map, MapMode, MapOptions, RenderMode};
use mln_renderer::UpdateReader;
use mln_storage::Kind;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::PropertyValue;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style_types::VisibilityType;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use support::*;

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

/// A frontend that loads resources asynchronously, but renders a frame only
/// when the test asks, at the time it chooses.
#[derive(Clone)]
struct TimedHeadlessFrontend {
    frontend: HeadlessFrontend,
    updates: Rc<RefCell<Option<UpdateReader>>>,
}

impl TimedHeadlessFrontend {
    fn new() -> Self {
        TimedHeadlessFrontend {
            frontend: HeadlessFrontend::with_options(Size::new(256, 256), 1.0, false).expect("a headless frontend"),
            updates: Rc::default(),
        }
    }

    fn render_at(&self, time: Instant) -> RenderResult {
        let update = self.updates.borrow().as_ref().and_then(UpdateReader::latest).expect("update parameters");
        let mut parameters = (*update.parameters).clone();
        parameters.time_point = Some(time);
        let parameters = Arc::new(parameters);
        let _scope = BackendScope::new(self.frontend.get_backend(), ScopeType::Explicit);
        if let Some(frames) = self.frontend.target().frames() {
            self.frontend.with_renderer(|renderer| renderer.render(&parameters, frames));
        }
        RenderResult { image: self.frontend.read_still_image(), stats: stats(&self.frontend) }
    }
}

/// A frontend that renders the map it's attached to.
trait Frontend: Clone + 'static {
    fn attach(&self, map: &Map);
}

impl Frontend for HeadlessFrontend {
    fn attach(&self, map: &Map) {
        HeadlessFrontend::attach(self, map);
    }
}

impl Frontend for TimedHeadlessFrontend {
    fn attach(&self, map: &Map) {
        self.frontend.attach(map);
        *self.updates.borrow_mut() = Some(map.update_reader());
    }
}

/// The rendering stats of the frontend's last frame.
fn stats(frontend: &HeadlessFrontend) -> RenderingStats {
    frontend.get_backend().get_context().rendering_stats().clone()
}

struct SymbolRenderingTest<F> {
    _events: AsyncTask,
    run_loop: RunLoop,
    frontend: F,
    observer: Rc<StubMapObserver>,
    map: Rc<RefCell<Map>>,
}

impl SymbolRenderingTest<HeadlessFrontend> {
    fn new(count: usize, spacing: f64, kind: SymbolKind, mode: MapMode) -> Self {
        Self::with_frontend(
            || HeadlessFrontend::new(Size::new(256, 256), 1.0).expect("a headless frontend"),
            count,
            spacing,
            kind,
            mode,
        )
    }

    fn render(&self) -> RenderResult {
        let result = self.frontend.clone().render(&mut self.map.borrow_mut()).expect("render");
        self.observer.dispatch_events(&self.map);
        result
    }
}

impl<F: Frontend> SymbolRenderingTest<F> {
    /// The frontend's tasks run on the test's run loop, so `frontend` makes
    /// it after the loop exists.
    fn with_frontend(
        frontend: impl FnOnce() -> F,
        count: usize,
        spacing: f64,
        kind: SymbolKind,
        mode: MapMode,
    ) -> Self {
        let run_loop = RunLoop::new();
        let frontend = frontend();
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        file_source.set_response(Kind::Glyphs, |_| Some(file_response("test/fixtures/resources/glyphs.pbf")));
        let observer = Rc::new(StubMapObserver::default());
        let mut map = Map::with_file_source(
            file_source,
            MapOptions { map_mode: mode, size: Size::new(256, 256), ..Default::default() },
        )
        .unwrap();
        frontend.attach(&map);
        map.style_mut().load_json(&colliding_symbols_style(count, spacing, kind));
        map.style_mut().add_image(marker(kind == SymbolKind::SdfIcon));
        map.jump_to(&CameraOptions { center: Some(lat_lng(5.0, 5.0)), zoom: Some(4.0), ..Default::default() });
        let map = Rc::new(RefCell::new(map));
        let events = observer.observe(&map);
        SymbolRenderingTest { _events: events, run_loop, frontend, observer, map }
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
fn run_until(run_loop: &RunLoop, done: impl Fn() -> bool) {
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
        let frontend = test.frontend.clone();
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
                    *initial_image.borrow_mut() = Some(frontend.read_still_image());
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
                    if initial_image.borrow().as_ref() != Some(&frontend.read_still_image()) {
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
    let test =
        SymbolRenderingTest::with_frontend(TimedHeadlessFrontend::new, 2, 0.5, SymbolKind::Icon, MapMode::Continuous);
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
        test.frontend.render_at(start + Duration::from_millis(ms))
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

    let mut blocker = SymbolLayer::new("blocker", "points");
    blocker.set_icon_image(&PropertyValue::Constant(mln_style::expression::Image::new("marker")));
    test.map.borrow_mut().style_mut().add_layer(Box::new(blocker), None).unwrap();
    let hidden = test.render();
    assert_eq!(hidden.stats.num_draw_calls, 1);
    assert_eq!(initial.image, hidden.image);

    test.map
        .borrow_mut()
        .style_mut()
        .get_layer_mut("blocker")
        .expect("the blocker layer")
        .set_visibility(VisibilityType::None);
    let visible = test.render();
    assert_eq!(visible.stats.num_draw_calls, 1);
    assert_eq!(initial.image, visible.image);
}
