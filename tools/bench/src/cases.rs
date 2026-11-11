//! The benchmark cases (README.md). The basemap's frame and load cases
//! follow maplibre-native-ffi's of the same names, over the same data and
//! cameras, so their numbers are comparable to the C++ core's.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use mln_map::RenderMode;
use mln_text::GlyphCache;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::latency::{set_tile_latency, take_tile_requests};
use crate::meter::{Meter, live_heap_mib, ms};
use crate::scene::{
    BASEMAP_SIZE, BASEMAP_ZOOM, CJK_SIZE, CJK_STYLE, CJK_ZOOM, Mode, REAL_WORLD, REAL_WORLD_SIZE, REFRESH, Scene,
    basemap_center, basemap_north_west, basemap_point, basemap_south_east, basemap_style, cjk_center, write_frame,
};
use crate::stats::{median, percentile};
use crate::{Context, Metrics};

pub struct Case {
    pub name: String,
    run: Box<dyn Fn(&Context) -> Metrics>,
}

impl Case {
    fn new(name: impl Into<String>, run: impl Fn(&Context) -> Metrics + 'static) -> Case {
        Case { name: name.into(), run: Box::new(run) }
    }

    pub fn run(&self, cx: &Context) -> Metrics {
        (self.run)(cx)
    }
}

/// A style at a camera: where a scene's still and load cases look.
#[derive(Clone)]
struct View {
    name: &'static str,
    size: Size,
    center: LatLng,
    zoom: f64,
    style: Style,
}

#[derive(Clone)]
enum Style {
    Url(String),
    Json(&'static str),
    Basemap,
}

impl View {
    fn scene(&self, cx: &Context, mode: Mode) -> Scene {
        let mut scene = Scene::new(self.size, mode);
        scene.jump(self.center, self.zoom);
        self.load(cx, &mut scene);
        scene
    }

    /// Starts loading the style on `scene`.
    fn load(&self, cx: &Context, scene: &mut Scene) {
        match &self.style {
            Style::Url(url) => scene.load_url(url),
            Style::Json(json) => scene.load_json(json),
            Style::Basemap => scene.load_json(&basemap_style(&cx.fixtures)),
        }
    }
}

fn views() -> Vec<View> {
    let mut views: Vec<View> = REAL_WORLD
        .iter()
        .map(|scene| View {
            name: scene.name,
            size: REAL_WORLD_SIZE,
            center: scene.center(),
            zoom: scene.zoom,
            style: Style::Url(scene.style_url()),
        })
        .collect();
    views.push(View {
        name: "basemap",
        size: BASEMAP_SIZE,
        center: basemap_center(),
        zoom: BASEMAP_ZOOM,
        style: Style::Basemap,
    });
    views
}

/// Every case, in the order they run.
pub fn all() -> Vec<Case> {
    let mut cases = Vec::new();
    for view in views() {
        let name = format!("Still/{}", view.name);
        cases.push(Case::new(name, move |cx| still(cx, &view)));
    }
    for (icons, overlap) in [(256, false), (1024, false), (1024, true)] {
        let name = format!("Still/CollidingIcons/{icons}{}", if overlap { "-overlap" } else { "" });
        cases.push(Case::new(name.clone(), move |cx| still_colliding_icons(cx, &name, icons, overlap)));
    }
    cases.push(Case::new("Frame/Throughput", frame_throughput));
    cases.push(Case::new("Frame/Paced", frame_paced));
    cases.push(Case::new("Frame/Oscillate", |cx| frame_oscillate(cx, "Frame/Oscillate", rotate_swing)));
    cases.push(Case::new("Frame/OscillatePanZoom", |cx| frame_oscillate(cx, "Frame/OscillatePanZoom", pan_zoom_swing)));
    cases.push(Case::new("Frame/Idle", frame_idle));
    cases.push(Case::new("Frame/SettleWithoutSymbols", frame_settle_without_symbols));
    cases.push(Case::new("Frame/AddImages", frame_add_images));
    cases.push(Case::new("Frame/FeatureState", frame_feature_state));
    cases.push(Case::new("Frame/FeatureStateHover", frame_feature_state_hover));
    cases.push(Case::new("Frame/HideLayers", frame_hide_layers));
    for (name, data_driven) in [("Frame/Extrusions", false), ("Frame/Extrusions/DataDriven", true)] {
        cases.push(Case::new(name, move |cx| frame_extrusions(cx, name, data_driven)));
    }
    for view in views() {
        let name = format!("Load/Style/{}", view.name);
        cases.push(Case::new(name, move |cx| load_style(cx, &view)));
    }
    cases.push(Case::new("Load/Concurrent", load_concurrent));
    cases.push(Case::new("Load/GlyphCache/Private", |cx| load_glyph_cache(cx, false)));
    cases.push(Case::new("Load/GlyphCache/Shared", |cx| load_glyph_cache(cx, true)));
    cases.push(Case::new("Load/JumpWarm", |cx| load_jump(cx, false)));
    cases.push(Case::new("Load/JumpCold", |cx| load_jump(cx, true)));
    cases
}

/// A still frame of a map whose tiles and pipelines are ready, read back,
/// the camera alternating between two zooms 0.001 apart so that each is a
/// new frame over the same tiles.
fn still(cx: &Context, view: &View) -> Metrics {
    let mut scene = view.scene(cx, Mode::Still);
    let nudge = |scene: &mut Scene, i: usize| scene.jump(view.center, view.zoom + 0.001 * (i % 2) as f64);
    for i in 0..10 {
        nudge(&mut scene, i);
        scene.render_still();
    }
    let mut meter = Meter::new();
    for i in 0..cx.iterations(100) {
        meter.time(|| {
            nudge(&mut scene, i);
            scene.render_still()
        });
    }
    if let Some(dir) = &cx.frames {
        nudge(&mut scene, 0);
        write_frame(dir, &format!("Still/{}", view.name), &scene.render_still());
    }
    meter.finish()
}

/// A still frame of `icons` icons at one point, each with its own sort key,
/// so that each is a drawable of its own. Without `overlap`, placement
/// hides all but one.
fn still_colliding_icons(cx: &Context, name: &str, icons: usize, overlap: bool) -> Metrics {
    let features: Vec<String> = (0..icons)
        .map(|i| {
            format!(
                r#"{{"type":"Feature","properties":{{"rank":{i}}},"geometry":{{"type":"Point","coordinates":[5,5]}}}}"#
            )
        })
        .collect();
    let style = format!(
        r#"{{"version":8,"sources":{{"points":{{"type":"geojson","maxzoom":0,
            "data":{{"type":"FeatureCollection","features":[{}]}}}}}},
            "layers":[{{"id":"symbols","type":"symbol","source":"points","layout":{{
            "icon-image":"marker","symbol-sort-key":["get","rank"],"icon-allow-overlap":{overlap}}}}}]}}"#,
        features.join(",")
    );
    let center = LatLng::new(5.0, 5.0).expect("a center");
    let mut scene = Scene::new(Size::new(256, 256), Mode::Still);
    scene.jump(center, 4.0);
    scene.load_json(&style);
    scene.add_image("marker", PremultipliedImage::from_vec(Size::new(16, 16), vec![255; 16 * 16 * 4]));
    let nudge = |scene: &mut Scene, i: usize| scene.jump(center, 4.0 + 0.001 * (i % 2) as f64);
    for i in 0..10 {
        nudge(&mut scene, i);
        scene.render_still();
    }
    let mut meter = Meter::new();
    for i in 0..cx.iterations(100) {
        meter.time(|| {
            nudge(&mut scene, i);
            scene.render_still()
        });
    }
    if let Some(dir) = &cx.frames {
        nudge(&mut scene, 0);
        write_frame(dir, name, &scene.render_still());
    }
    meter.finish()
}

/// From loading the style on a new map to its first complete still: the
/// style, sprite and glyph requests, tile parsing and layout, pipeline
/// creation and the frame. The GPU device is set up before.
fn load_style(cx: &Context, view: &View) -> Metrics {
    let mut meter = Meter::new();
    let mut last = None;
    for _ in 0..cx.iterations(10) {
        let mut scene = Scene::new(view.size, Mode::Still);
        scene.jump(view.center, view.zoom);
        meter.start();
        view.load(cx, &mut scene);
        last = Some(scene.render_still());
        meter.stop();
    }
    if let (Some(dir), Some(image)) = (&cx.frames, &last) {
        write_frame(dir, &format!("Load/Style/{}", view.name), image);
    }
    meter.finish()
}

/// Renderers on threads of their own, as the render tests run many at
/// once: each loads a real-world style on a new map and renders its first
/// complete still through `HeadlessFrontend::render`, the threads taking
/// the styles in turn.
fn load_concurrent(cx: &Context) -> Metrics {
    let views: Vec<View> = views().into_iter().filter(|view| matches!(view.style, Style::Url(_))).collect();
    concurrent(cx, "Load/Concurrent", &views, false)
}

/// Load/Concurrent over the CJK view, every renderer with a glyph cache of
/// its own or all sharing one: the glyphs that several maps in one process
/// parse from the same glyph ranges.
fn load_glyph_cache(cx: &Context, shared: bool) -> Metrics {
    let view =
        View { name: "cjk", size: CJK_SIZE, center: cjk_center(), zoom: CJK_ZOOM, style: Style::Json(CJK_STYLE) };
    let name = if shared { "Load/GlyphCache/Shared" } else { "Load/GlyphCache/Private" };
    concurrent(cx, name, &[view], shared)
}

/// 16 renderers on threads of their own, taking `views` in turn: each loads
/// its view's style on a new map and renders its first complete still
/// through `HeadlessFrontend::render`, as the render tests' workers draw.
/// An iteration is every renderer's load and still; the GPU devices are set
/// up before. With `shared`, the renderers share one glyph cache, new for
/// each iteration.
fn concurrent(cx: &Context, name: &str, views: &[View], shared: bool) -> Metrics {
    const RENDERERS: usize = 16;
    let mut meter = Meter::new();
    let mut last = None;
    for _ in 0..cx.iterations(5) {
        let glyph_cache = shared.then(Arc::<GlyphCache>::default);
        let ready = Barrier::new(RENDERERS + 1);
        let images: Vec<PremultipliedImage> = std::thread::scope(|scope| {
            let threads: Vec<_> = views
                .iter()
                .cycle()
                .take(RENDERERS)
                .map(|view| {
                    let (ready, glyph_cache) = (&ready, glyph_cache.clone());
                    scope.spawn(move || {
                        let mut scene =
                            Scene::with_glyph_cache(view.size, Mode::Still, glyph_cache.unwrap_or_default());
                        scene.jump(view.center, view.zoom);
                        ready.wait();
                        view.load(cx, &mut scene);
                        scene.render()
                    })
                })
                .collect();
            ready.wait();
            meter.start();
            let images = threads.into_iter().map(|thread| thread.join().expect("a renderer")).collect();
            meter.stop();
            images
        });
        last = images.into_iter().next();
    }
    if let (Some(dir), Some(image)) = (&cx.frames, &last) {
        write_frame(dir, name, image);
    }
    meter.finish()
}

fn basemap(cx: &Context, mode: Mode) -> Scene {
    let mut scene = Scene::new(BASEMAP_SIZE, mode);
    scene.jump(basemap_center(), BASEMAP_ZOOM);
    scene.load_json(&basemap_style(&cx.fixtures));
    scene
}

/// The camera around a circle of 200 pixels around the basemap's center,
/// at `index` of `lap` steps; the circle stays inside the block's tiles.
fn circle(index: usize, lap: usize) -> LatLng {
    let center = basemap_center();
    // Pixels per degree at the zoom, for 512-pixel tiles.
    let per_degree_x = 512.0 * BASEMAP_ZOOM.exp2() / 360.0;
    let per_degree_y = per_degree_x / center.latitude().to_radians().cos();
    let angle = std::f64::consts::TAU * index as f64 / lap as f64;
    LatLng::new(
        center.latitude() + 200.0 * angle.sin() / per_degree_y,
        center.longitude() + 200.0 * angle.cos() / per_degree_x,
    )
    .expect("a point of the circle")
}

/// Frames as fast as the host takes them: from a camera jump around the
/// circle until the GPU has finished the frame that shows it, over tiles an
/// untimed lap loaded.
fn frame_throughput(cx: &Context) -> Metrics {
    const LAP: usize = 240;
    let mut scene = basemap(cx, Mode::OnDemand);
    scene.settle();
    for index in 0..LAP {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        scene.pump();
        scene.render_frame();
    }
    scene.settle();
    let mut meter = Meter::new();
    for index in 0..cx.iterations(300) {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        meter.time(|| {
            scene.pump();
            scene.render_frame()
        });
    }
    let (p95, p99) = (percentile(meter.times(), 95.0), percentile(meter.times(), 99.0));
    let mut metrics = meter.finish();
    metrics.lead("frame_p95_ms", p95);
    metrics.lead("frame_p99_ms", p99);
    metrics
}

/// Frames of Frame/FeatureState's map, with one building's state changing
/// in every frame, as hover highlighting does.
fn frame_feature_state_hover(cx: &Context) -> Metrics {
    const LAP: usize = 240;
    let mut style: serde_json::Value =
        serde_json::from_str(&basemap_style(&cx.fixtures)).expect("the basemap style is JSON");
    let layers = style["layers"].as_array_mut().expect("the style has layers");
    let buildings = layers.iter_mut().find(|layer| layer["id"] == "buildings").expect("the buildings layer");
    buildings["paint"]["fill-color"] =
        serde_json::json!(["case", ["boolean", ["feature-state", "selected"], false], "#c9a28f", "#d9d0c9"]);
    let mut scene = Scene::new(BASEMAP_SIZE, Mode::OnDemand);
    scene.jump(basemap_center(), BASEMAP_ZOOM);
    scene.load_json(&style.to_string());
    scene.settle();
    for index in 0..LAP {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        scene.pump();
        scene.render_frame();
    }
    scene.settle();
    let ids = scene.feature_ids("basemap", "buildings");
    assert!(!ids.is_empty(), "buildings has no features with ids");
    let mut meter = Meter::new();
    for index in 0..cx.iterations(100) {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        scene.select_feature("basemap", "buildings", &ids[index * 7919 % ids.len()], index % 2 == 0);
        meter.time(|| {
            scene.pump();
            scene.render_frame()
        });
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, "Frame/FeatureStateHover", &scene.read_frame());
    }
    let (p95, p99) = (percentile(meter.times(), 95.0), percentile(meter.times(), 99.0));
    let mut metrics = meter.finish();
    metrics.lead("frame_p95_ms", p95);
    metrics.lead("frame_p99_ms", p99);
    metrics
}

/// Frames of Frame/Throughput over the basemap with its buildings and a
/// street overlay colored by feature state, after every building and street
/// of the loaded tiles was given a state; no state changes while it pans.
fn frame_feature_state(cx: &Context) -> Metrics {
    const LAP: usize = 240;
    const SOURCE_LAYERS: [&str; 2] = ["buildings", "streets"];
    let mut style: serde_json::Value =
        serde_json::from_str(&basemap_style(&cx.fixtures)).expect("the basemap style is JSON");
    let selected =
        |on: &str, off: &str| serde_json::json!(["case", ["boolean", ["feature-state", "selected"], false], on, off]);
    let layers = style["layers"].as_array_mut().expect("the style has layers");
    let buildings = layers.iter_mut().find(|layer| layer["id"] == "buildings").expect("the buildings layer");
    buildings["paint"]["fill-color"] = selected("#c9a28f", "#d9d0c9");
    let streets = layers.iter().position(|layer| layer["source-layer"] == "streets").expect("a streets layer");
    layers.insert(
        streets,
        serde_json::json!({
            "id": "selected-streets",
            "type": "line",
            "source": "basemap",
            "source-layer": "streets",
            "paint": {"line-color": selected("#e55e5e", "rgba(0, 0, 0, 0)"), "line-width": 6},
        }),
    );
    let mut scene = Scene::new(BASEMAP_SIZE, Mode::OnDemand);
    scene.jump(basemap_center(), BASEMAP_ZOOM);
    scene.load_json(&style.to_string());
    scene.settle();
    for index in 0..LAP {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        scene.pump();
        scene.render_frame();
    }
    for source_layer in SOURCE_LAYERS {
        scene.select_every_feature("basemap", source_layer);
    }
    scene.settle();
    let mut meter = Meter::new();
    for index in 0..cx.iterations(100) {
        scene.jump(circle(index, LAP), BASEMAP_ZOOM);
        meter.time(|| {
            scene.pump();
            scene.render_frame()
        });
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, "Frame/FeatureState", &scene.read_frame());
    }
    let (p95, p99) = (percentile(meter.times(), 95.0), percentile(meter.times(), 99.0));
    let mut metrics = meter.finish();
    metrics.lead("frame_p95_ms", p95);
    metrics.lead("frame_p99_ms", p99);
    metrics
}

/// Every layer of the loaded basemap hidden until the map settles, then
/// shown again until it settles complete. `hidden_heap_mib` and
/// `hidden_gpu_mib` are the heap and the GPU buffers live while they're
/// hidden.
fn frame_hide_layers(cx: &Context) -> Metrics {
    let mut scene = basemap(cx, Mode::OnDemand);
    scene.settle();
    let mut meter = Meter::new();
    let (mut heap, mut gpu) = (Vec::new(), Vec::new());
    for _ in 0..cx.iterations(10) {
        meter.start();
        scene.set_source_visible("basemap", false);
        scene.settle();
        heap.push(live_heap_mib());
        gpu.push(scene.gpu_buffers_mib());
        scene.set_source_visible("basemap", true);
        scene.settle();
        meter.stop();
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, "Frame/HideLayers", &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.lead("hidden_heap_mib", median(&heap));
    metrics.lead("hidden_gpu_mib", median(&gpu));
    metrics
}

/// The basemap's buildings extruded, over its background, with every
/// building's height and color from its id if `data_driven`.
fn extrusions_style(fixtures: &std::path::Path, data_driven: bool) -> String {
    let mut style: serde_json::Value =
        serde_json::from_str(&basemap_style(fixtures)).expect("the basemap style is JSON");
    let (height, color) = if data_driven {
        (
            serde_json::json!(["+", 8, ["*", 4, ["%", ["id"], 10]]]),
            serde_json::json!(["case", ["==", ["%", ["id"], 2], 0], "#c9a28f", "#d9d0c9"]),
        )
    } else {
        (serde_json::json!(20), serde_json::json!("#d9d0c9"))
    };
    style["layers"] = serde_json::json!([
        { "id": "background", "type": "background", "paint": { "background-color": "#f2efe9" } },
        {
            "id": "buildings",
            "type": "fill-extrusion",
            "source": "basemap",
            "source-layer": "buildings",
            "paint": { "fill-extrusion-height": height, "fill-extrusion-color": color, "fill-extrusion-opacity": 0.9 }
        }
    ]);
    style.to_string()
}

/// Frames of the basemap's buildings extruded (`extrusions_style`), seen at
/// a pitch, the zoom alternating by 0.001 as in `Still`. `gpu_mib` is the
/// vertex and index buffers the GPU holds for them.
fn frame_extrusions(cx: &Context, name: &str, data_driven: bool) -> Metrics {
    let nudge = |scene: &mut Scene, i: usize| {
        scene.jump_pitched(basemap_center(), BASEMAP_ZOOM + 0.5 + 0.001 * (i % 2) as f64, 30.0, 50.0);
    };
    let mut scene = Scene::new(BASEMAP_SIZE, Mode::OnDemand);
    nudge(&mut scene, 0);
    scene.load_json(&extrusions_style(&cx.fixtures, data_driven));
    scene.settle();
    let gpu = scene.gpu_buffers_mib();
    let mut meter = Meter::new();
    for i in 0..cx.iterations(100) {
        nudge(&mut scene, i);
        meter.time(|| scene.render_frame());
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, name, &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.lead("gpu_mib", gpu);
    metrics
}

/// The flight of Frame/Paced: from the block's north-west corner at zoom
/// 14 to its south-east corner at zoom 15.5, eased in and out, at
/// `progress` from 0 to 1. The tiles stop at zoom 14, so from zoom 15 on the
/// map lays the same data out again as zoom-15 tiles, as it does past a
/// basemap's deepest zoom.
fn fly(scene: &mut Scene, progress: f64) {
    let eased = progress * progress * (3.0 - 2.0 * progress);
    let (from, to) = (basemap_north_west(), basemap_south_east());
    let center = LatLng::new(
        from.latitude() + (to.latitude() - from.latitude()) * eased,
        from.longitude() + (to.longitude() - from.longitude()) * eased,
    )
    .expect("a point of the flight");
    scene.jump(center, BASEMAP_ZOOM + 1.5 * eased);
}

/// A two-second camera flight on a 60 Hz display's clock, with only the
/// first view's tiles loaded, until the final view shows complete. At each
/// refresh with no frame in flight, the host moves the camera to where the
/// animation will be at the next refresh and draws a frame; the frame is
/// shown at the first refresh after the GPU finishes it, and the next one
/// starts there. Refreshes are absolute deadlines, and the camera follows
/// the refresh count, so every run draws the same cameras however slow.
fn frame_paced(cx: &Context) -> Metrics {
    let ticks = (Duration::from_secs(2).as_secs_f64() / REFRESH.as_secs_f64()).ceil() as usize;
    let mut scene = basemap(cx, Mode::OnDemand);
    let mut meter = Meter::new();
    let (mut frames, mut missed, mut partial) = (Vec::new(), 0, 0);
    let (mut stalls, mut complete_after) = (Vec::new(), Vec::new());
    for _ in 0..cx.iterations(3) {
        scene.clear_data();
        fly(&mut scene, 0.0);
        scene.settle();
        meter.start();
        let start = Instant::now();
        let refresh = |tick: usize| start + REFRESH * tick as u32;
        let (mut tick, mut shown, mut longest) = (0, 0, 0);
        let mut complete = false;
        while tick < ticks {
            std::thread::sleep(refresh(tick).saturating_duration_since(Instant::now()));
            scene.pump();
            let progress = ((tick + 1) as f64 / ticks as f64).min(1.0);
            fly(&mut scene, progress);
            let demanded = Instant::now();
            let status = scene.render_frame();
            let finished = Instant::now();
            frames.push(ms(finished - demanded));
            missed += usize::from(finished > refresh(tick + 1));
            partial += usize::from(status.mode != RenderMode::Full);
            complete = progress == 1.0 && status.mode == RenderMode::Full;
            // Shown at the first refresh after it finished, where the next
            // frame starts.
            let next = ((finished - start).as_secs_f64() / REFRESH.as_secs_f64()).ceil() as usize;
            longest = longest.max(next - shown);
            (shown, tick) = (next, next);
        }
        if !complete {
            fly(&mut scene, 1.0);
            scene.present_until_complete();
        }
        complete_after.push(ms(Instant::now().saturating_duration_since(refresh(ticks))));
        stalls.push(ms(REFRESH * longest as u32));
        meter.stop();
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, "Frame/Paced", &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.demote("time_ms");
    metrics.other("frame_p50_ms", percentile(&frames, 50.0));
    metrics.lead("frame_p95_ms", percentile(&frames, 95.0));
    metrics.lead("frame_p99_ms", percentile(&frames, 99.0));
    metrics.lead("missed_pct", 100.0 * missed as f64 / frames.len() as f64);
    metrics.lead("partial_pct", 100.0 * partial as f64 / frames.len() as f64);
    metrics.lead("longest_stall_ms", median(&stalls));
    metrics.lead("complete_after_ms", median(&complete_after));
    metrics.other("frames", frames.len() as f64 / complete_after.len() as f64);
    metrics
}

/// A swing back and forth from 0 at `seconds`: to 1, then between -1 and
/// 1, each swing half a second, back to 0 after each second.
fn swing(seconds: f64) -> f64 {
    4.0 * ((seconds - 0.25).rem_euclid(1.0) - 0.5).abs() - 1.0
}

/// A camera of the basemap's: its center, zoom and bearing.
type Camera = (LatLng, f64, f64);

/// Frame/Oscillate's camera at `seconds`: the bearing swings between -90°
/// and 90°. The center is 0.23 of a tile south-east of a tile corner, so
/// that the view reaches the column and the row of tiles two west and two
/// north of that corner only near its widest, around 37° and 53°: they
/// come into view for about a sixth of a second's swing each time one
/// passes, less than a tile takes to load.
fn rotate_swing(seconds: f64) -> Camera {
    (basemap_point(2620.23, 6332.23), BASEMAP_ZOOM, 90.0 * swing(seconds))
}

/// Frame/OscillatePanZoom's camera at `seconds`: the center swings 0.65 of
/// a tile east and west of a tile's center while the zoom goes from 14.9
/// out to 14 at each end, so that the view's edges reach the next tiles
/// only in the last eighth of a swing.
fn pan_zoom_swing(seconds: f64) -> Camera {
    let swing = swing(seconds);
    (basemap_point(2620.5 + 0.65 * swing, 6332.0), BASEMAP_ZOOM + 0.9 * (1.0 - swing.abs()), 0.0)
}

/// Three seconds of a camera swinging back and forth (`path`) on a 60 Hz
/// display's clock, as a drag back and forth does, over the basemap, each
/// tile request answered 100 ms late, as over a network
/// ([`set_tile_latency`]). Only the first view is loaded at the start; the
/// frames are paced as in Frame/Paced. The tile requests are the tiles it
/// loads, and loads again after dropping them.
fn frame_oscillate(cx: &Context, name: &str, path: fn(f64) -> Camera) -> Metrics {
    const LATENCY: Duration = Duration::from_millis(100);
    const SECONDS: f64 = 3.0;
    let ticks = (SECONDS / REFRESH.as_secs_f64()).round() as usize;
    let jump = |scene: &mut Scene, seconds: f64| {
        let (center, zoom, bearing) = path(seconds);
        scene.jump_rotated(center, zoom, bearing);
    };
    let mut scene = basemap(cx, Mode::OnDemand);
    set_tile_latency(LATENCY);
    let mut meter = Meter::new();
    let (mut frames, mut partial, mut requests) = (Vec::new(), 0, Vec::new());
    for _ in 0..cx.iterations(3) {
        scene.clear_data();
        jump(&mut scene, 0.0);
        scene.settle();
        take_tile_requests();
        meter.start();
        let start = Instant::now();
        let refresh = |tick: usize| start + REFRESH * tick as u32;
        let mut tick = 0;
        while tick < ticks {
            std::thread::sleep(refresh(tick).saturating_duration_since(Instant::now()));
            scene.pump();
            jump(&mut scene, (tick + 1).min(ticks) as f64 / ticks as f64 * SECONDS);
            let demanded = Instant::now();
            let status = scene.render_frame();
            let finished = Instant::now();
            frames.push(ms(finished - demanded));
            partial += usize::from(status.mode != RenderMode::Full);
            // Shown at the first refresh after it finished, where the next
            // frame starts.
            let next = ((finished - start).as_secs_f64() / REFRESH.as_secs_f64()).ceil() as usize;
            tick = next.max(tick + 1);
        }
        meter.stop();
        requests.push(take_tile_requests() as f64);
    }
    set_tile_latency(Duration::ZERO);
    if let Some(dir) = &cx.frames {
        scene.settle();
        write_frame(dir, name, &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.demote("time_ms");
    metrics.lead("tile_requests", median(&requests));
    metrics.lead("partial_pct", 100.0 * partial as f64 / frames.len() as f64);
    metrics.other("frame_p50_ms", percentile(&frames, 50.0));
    metrics.lead("frame_p95_ms", percentile(&frames, 95.0));
    metrics.lead("frame_p99_ms", percentile(&frames, 99.0));
    metrics.other("frame_max_ms", percentile(&frames, 100.0));
    metrics.other("frames", frames.len() as f64 / requests.len() as f64);
    metrics
}

/// A settled map in slices of 100 ms, the run loop drawing whenever the map
/// asks, as an idle app's: what it costs to do nothing, and how many frames
/// it draws when nothing changes.
fn frame_idle(cx: &Context) -> Metrics {
    const SLICE: Duration = Duration::from_millis(100);
    let scene = basemap(cx, Mode::Driven);
    let start = Instant::now();
    while !scene.is_settled() {
        scene.run_for(REFRESH);
        assert!(start.elapsed() < Duration::from_secs(60), "the basemap doesn't settle");
    }
    let frames_before = scene.frames();
    let slices = cx.iterations(10);
    let mut meter = Meter::new();
    for _ in 0..slices {
        meter.time(|| scene.run_for(SLICE));
    }
    let frames = (scene.frames() - frames_before) as f64;
    let mut metrics = meter.finish();
    metrics.demote("time_ms");
    metrics.lead("frames_per_s", frames / (SLICE.as_secs_f64() * slices as f64));
    metrics
}

/// From a camera nudge on the basemap with its symbol layers left out, which
/// has nothing to place, until the map stops asking for frames: the frames
/// drawn and the time. The nudge stays over the loaded tiles.
fn frame_settle_without_symbols(cx: &Context) -> Metrics {
    let mut style: serde_json::Value =
        serde_json::from_str(&basemap_style(&cx.fixtures)).expect("the basemap style is JSON");
    style["layers"].as_array_mut().expect("the style has layers").retain(|layer| layer["type"] != "symbol");
    let mut scene = Scene::new(BASEMAP_SIZE, Mode::Driven);
    scene.jump(basemap_center(), BASEMAP_ZOOM);
    scene.load_json(&style.to_string());
    // Until a frame after `before` finishes without asking for another.
    let settle = |scene: &Scene, before: u64| {
        let start = Instant::now();
        while scene.frames() == before || !scene.is_settled() {
            scene.run_for(REFRESH);
            assert!(start.elapsed() < Duration::from_secs(60), "the basemap doesn't settle");
        }
    };
    settle(&scene, 0);
    let mut meter = Meter::new();
    let mut frames = Vec::new();
    for index in 0..cx.iterations(10) {
        let before = scene.frames();
        meter.start();
        scene.jump(basemap_center(), BASEMAP_ZOOM + 0.001 * (index % 2) as f64);
        settle(&scene, before);
        meter.stop();
        frames.push((scene.frames() - before) as f64);
    }
    let mut metrics = meter.finish();
    metrics.lead("frames", median(&frames));
    metrics
}

/// From adding the first of 50 icons to the basemap, five on each refresh
/// over ten refreshes, as a host supplies marker icons when they load,
/// until the map stops asking for frames. A symbol layer over 20 points
/// draws five of the icons, which its tile asked for before they were
/// added; nothing draws the other 45. Each iteration loads the style again
/// first, untimed, without the icons.
fn frame_add_images(cx: &Context) -> Metrics {
    const ICONS: usize = 50;
    const PER_REFRESH: usize = 5;
    const DRAWN: usize = 5;
    let mut style: serde_json::Value =
        serde_json::from_str(&basemap_style(&cx.fixtures)).expect("the basemap style is JSON");
    let features: Vec<serde_json::Value> = (0..20)
        .map(|n| {
            let point = basemap_point(2619.5 + 0.1 * (n % 5) as f64, 6331.8 + 0.3 * (n / 5) as f64);
            serde_json::json!({
                "type": "Feature",
                "properties": {"icon": format!("marker-{}", n % DRAWN)},
                "geometry": {"type": "Point", "coordinates": [point.longitude(), point.latitude()]},
            })
        })
        .collect();
    style["sources"]["markers"] =
        serde_json::json!({"type": "geojson", "data": {"type": "FeatureCollection", "features": features}});
    style["layers"].as_array_mut().expect("the style has layers").push(serde_json::json!({
        "id": "markers",
        "type": "symbol",
        "source": "markers",
        "layout": {"icon-image": ["get", "icon"], "icon-allow-overlap": true},
    }));
    let style = style.to_string();
    let icon = |n: usize| {
        let pixel = [(n * 5) as u8, 0, 255 - (n * 5) as u8, 255];
        PremultipliedImage::from_vec(Size::new(24, 24), pixel.repeat(24 * 24))
    };
    let mut scene = Scene::new(BASEMAP_SIZE, Mode::OnDemand);
    scene.jump(basemap_center(), BASEMAP_ZOOM);
    let mut meter = Meter::new();
    let mut frames = Vec::new();
    for _ in 0..cx.iterations(5) {
        scene.load_json(&style);
        scene.settle();
        meter.start();
        let start = Instant::now();
        for tick in 0..ICONS / PER_REFRESH {
            std::thread::sleep((start + REFRESH * tick as u32).saturating_duration_since(Instant::now()));
            for n in tick * PER_REFRESH..(tick + 1) * PER_REFRESH {
                scene.add_image(&format!("marker-{n}"), icon(n));
            }
            let demanded = Instant::now();
            scene.pump();
            scene.render_frame();
            frames.push(ms(demanded.elapsed()));
        }
        scene.settle();
        meter.stop();
    }
    if let Some(dir) = &cx.frames {
        write_frame(dir, "Frame/AddImages", &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.lead("frame_p95_ms", percentile(&frames, 95.0));
    metrics.lead("frame_p99_ms", percentile(&frames, 99.0));
    metrics.other("frame_p50_ms", percentile(&frames, 50.0));
    metrics
}

/// From a jump between two views of the basemap that share no tile until
/// the first frame that shows the new view complete has finished on the
/// GPU, with frames on the display's clock. Both views load once before;
/// `cold` drops the loaded tiles before each jump, so it parses and lays
/// them out again.
fn load_jump(cx: &Context, cold: bool) -> Metrics {
    let views = [basemap_south_east(), basemap_north_west()];
    let mut scene = basemap(cx, Mode::OnDemand);
    for view in views {
        scene.jump(view, BASEMAP_ZOOM);
        scene.settle();
    }
    let mut meter = Meter::new();
    let mut frames = Vec::new();
    for index in 0..cx.iterations(10) {
        if cold {
            scene.clear_data();
        }
        meter.start();
        scene.jump(views[index % 2], BASEMAP_ZOOM);
        frames.push(scene.present_until_complete() as f64);
        meter.stop();
    }
    if let Some(dir) = &cx.frames {
        let name = if cold { "Load/JumpCold" } else { "Load/JumpWarm" };
        write_frame(dir, name, &scene.read_frame());
    }
    let mut metrics = meter.finish();
    metrics.other("frames", median(&frames));
    metrics
}
