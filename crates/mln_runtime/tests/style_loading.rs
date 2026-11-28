//! Styles that load from the cache and the network, and what a loaded style
//! reports and requests.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use mln_engine::camera::{CameraOptions, CameraUpdate};
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::storage::chrono::Timestamp;
use mln_engine::storage::{Kind, Response};
use mln_runtime::{Map, ResourceRequest, Runtime, RuntimeEventKind, RuntimeOptions};
use mln_style::geo::LatLng;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::support::still::{
    StillMap, StillOptions, assert_double_eq, center_pixel, check_image, file_response, ll, not_found, read_file, recv,
    serve, test_device,
};
use crate::support::{TempDir, TestRuntime, eventually, map, wait};

/// A runtime that keeps its cache in `dir`.
fn cached_runtime(dir: &TempDir) -> TestRuntime {
    let options = RuntimeOptions { cache_path: Some(dir.path("cache.db")), ..RuntimeOptions::default() };
    TestRuntime(Runtime::new(&options).expect("a runtime"))
}

/// `response`, which went stale at the epoch.
fn expired(mut response: Response) -> Response {
    response.caching.expires = Some(Timestamp::from_secs(0));
    response
}

/// The last segment of `url`'s path.
fn file_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// A provider that holds its requests until it opens, and then answers them
/// and those after with `answer`.
struct Gate<F> {
    /// Whether it opened, and the requests it holds until then.
    state: Mutex<(bool, Vec<ResourceRequest>)>,
    answer: F,
}

impl<F: Fn(&ResourceRequest) -> Response + Send + Sync + 'static> Gate<F> {
    /// Sets `runtime`'s provider to one that holds every request, and
    /// answers them with `answer` once [`open`](Self::open) is called.
    fn install(runtime: &Runtime, answer: F) -> Arc<Self> {
        let gate = Arc::new(Gate { state: Mutex::new((false, Vec::new())), answer });
        let provider = Arc::clone(&gate);
        wait(runtime.set_resource_provider(move |request: ResourceRequest| {
            let mut state = provider.state.lock();
            if state.0 {
                drop(state);
                let response = (provider.answer)(&request);
                let _ = request.respond(response);
            } else {
                state.1.push(request);
            }
        }));
        gate
    }

    /// Answers the requests held so far, and those after at once.
    fn open(&self) {
        let held = {
            let mut state = self.state.lock();
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        for request in held {
            let response = (self.answer)(&request);
            let _ = request.respond(response);
        }
    }
}

/// A style with no sources publishes an update once it loads: a continuous
/// map whose camera was set first (so the style's default camera doesn't
/// jump it) still renders the style and finishes loading
/// (maplibre/maplibre-native#4674).
#[test]
fn a_sourceless_style_loaded_after_the_camera_renders() {
    let map = StillMap::continuous();
    map.jump(CameraOptions { center: Some(ll(10.0, 20.0)), zoom: Some(3.0), ..Default::default() });
    map.load_json(
        r##"{"version": 8, "sources": {}, "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#00ff00"}}
        ]}"##,
    );

    map.settle();
    let finished = map.events.wait_for(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::LoadingFinished)));
    assert!(finished.is_some(), "the map never finished loading");
    assert_eq!(center_pixel(&map.image()), [0, 255, 0, 255]);
}

/// A style's default camera moves the map for the first style it loads, and
/// not for the styles after.
#[test]
fn set_style_default_camera() {
    let runtime = crate::support::runtime();
    let map = map(&runtime);
    let camera = |map: &Map| CameraOptions::from(wait(map.camera()).camera);

    wait(map.set_style_json(read_file("test/fixtures/api/empty.json")));
    let loaded = camera(&map);
    assert_double_eq(loaded.zoom.unwrap(), 0.0);
    assert_double_eq(loaded.pitch.unwrap(), 0.0);
    assert_double_eq(loaded.bearing.unwrap(), 0.0);
    assert_eq!(loaded.center.unwrap(), LatLng::default());

    wait(map.set_style_json(read_file("test/fixtures/api/empty-zoomed.json")));
    assert_double_eq(camera(&map).zoom.unwrap(), 0.0);

    let zoom: f64 = wait(map.style_root_property("zoom")).expect("the style's zoom").parse().unwrap();
    let update =
        CameraUpdate { camera: CameraOptions { zoom: Some(zoom), ..Default::default() }, ..Default::default() };
    wait(map.update_camera(update));
    assert_double_eq(camera(&map).zoom.unwrap(), 0.5);
}

/// A map shows what the cache holds while the network doesn't answer, even
/// once it has expired.
#[test]
fn offline() {
    // C++ CI leaves this test out on the macOS runners
    // (.github/workflows/macos-ci.yml, --gtest_filter=-...:Map.Offline). Their
    // VMs render through Metal's "Apple Paravirtual device", where a few
    // labels' glyphs land on other labels (image difference 0.0176), though
    // placement is the same as on an Apple GPU, where the image matches.
    let device = test_device().or_else(|| mln_runtime::Device::new(mln_runtime::DeviceOptions::default()).ok());
    let device_name = match device {
        #[cfg(feature = "webgpu")]
        Some(mln_runtime::Device::WebGpu(device)) => Some(device.adapter_info().name),
        #[cfg(all(feature = "metal", target_vendor = "apple"))]
        Some(mln_runtime::Device::Metal(device)) => Some(device.name()),
        _ => None,
    };
    if device_name.is_some_and(|name| name == "Apple Paravirtual device") {
        eprintln!("offline: skipped on the Apple Paravirtual device, as in C++ CI");
        return;
    }

    let style_url = "http://127.0.0.1:3000/style.json";
    let dir = TempDir::new();
    {
        // The cache gets expired copies of everything the map needs.
        let seeding = StillMap::on(cached_runtime(&dir), StillOptions::default());
        serve(&seeding.runtime, |resource| {
            let file = match file_name(&resource.url) {
                "sprite.json" | "sprite.png" | "streets.json" | "style.json" | "0-0-0.vector.pbf" => {
                    file_name(&resource.url).to_owned()
                }
                "0-255.pbf" => "glyph.pbf".to_owned(),
                _ => return not_found(),
            };
            expired(file_response(&format!("test/fixtures/map/offline/{file}")))
        });
        seeding.load_url(style_url);
        seeding.render();
    }

    // The network never answers: the gate never opens.
    let map = StillMap::on(cached_runtime(&dir), StillOptions::default());
    Gate::install(&map.runtime, |_: &ResourceRequest| not_found());
    map.load_url(style_url);
    check_image("test/fixtures/map/offline", &map.render(), 0.0015, 0.1);
}

const STYLE_URL: &str = "https://example.com/maps/test";

/// A runtime whose cache holds a copy of empty.json for [`STYLE_URL`] that
/// has expired, and the revalidations of that copy that its provider gets
/// after, unanswered.
fn expired_style_runtime(dir: &TempDir) -> (TestRuntime, Receiver<ResourceRequest>) {
    let runtime = cached_runtime(dir);
    let (sender, revalidations): (Sender<ResourceRequest>, _) = channel();
    let sender = Mutex::new(sender);
    wait(runtime.set_resource_provider(move |request: ResourceRequest| {
        // The cached copy is usable, so the revalidation carries its
        // validators rather than its data.
        if request.resource().prior_caching.expires.is_some() {
            sender.lock().send(request).unwrap();
        } else {
            let _ = request.respond(expired(file_response("test/fixtures/api/empty.json")));
        }
    }));
    // A map caches the expired copy. Released, it cancels its own
    // revalidations, which go: the tests answer their maps' alone.
    let caching = map(&runtime);
    wait(caching.set_style_url(STYLE_URL));
    wait(caching.release());
    while revalidations.try_recv().is_ok() {}
    (runtime, revalidations)
}

/// The next revalidation that a map that's still live waits for.
#[track_caller]
fn live_revalidation(revalidations: &Receiver<ResourceRequest>) -> ResourceRequest {
    loop {
        let revalidation = recv(revalidations);
        if !revalidation.is_cancelled() {
            return revalidation;
        }
    }
}

/// A fresh style with one layer, "revalidated".
fn revalidated_style() -> Response {
    let style = r#"{"version": 8, "sources": {}, "layers": [{"id": "revalidated", "type": "background"}]}"#;
    Response { data: Some(Arc::new(style.as_bytes().to_vec())), ..Response::default() }
}

/// A map that shows an expired style from the cache takes the style that
/// its revalidation brings, unless the style was edited before it came.
#[test]
fn style_expired() {
    let dir = TempDir::new();
    let (runtime, revalidations) = expired_style_runtime(&dir);
    let unedited = map(&runtime);
    wait(unedited.set_style_url(STYLE_URL));
    let revalidation = live_revalidation(&revalidations);
    revalidation.respond(revalidated_style()).unwrap();
    // The revalidated style loads on the runtime's thread, after the answer.
    assert!(eventually(|| wait(unedited.style_layer("revalidated")).is_some()), "the revalidated style loads");

    let dir = TempDir::new();
    let (runtime, revalidations) = expired_style_runtime(&dir);
    let edited = map(&runtime);
    wait(edited.set_style_url(STYLE_URL));
    let revalidation = live_revalidation(&revalidations);
    wait(edited.add_style_layer_json(r#"{"id": "bg", "type": "background"}"#, None));
    revalidation.respond(revalidated_style()).unwrap();
    assert!(wait(edited.style_layer("bg")).is_some());
    assert!(wait(edited.style_layer("revalidated")).is_none());
}

/// A projection set at runtime keeps the revalidation of an expired style
/// from replacing the style.
#[test]
fn style_expired_with_projection() {
    let dir = TempDir::new();
    let (runtime, revalidations) = expired_style_runtime(&dir);
    let map = map(&runtime);
    wait(map.set_style_url(STYLE_URL));
    let revalidation = live_revalidation(&revalidations);
    wait(map.set_style_root_property("projection", r#"{"type": "globe"}"#));
    revalidation.respond(revalidated_style()).unwrap();

    assert!(wait(map.style_layer("revalidated")).is_none());
    let projection = wait(map.style_root_property("projection")).expect("a projection");
    let projection: Value = serde_json::from_str(&projection).unwrap();
    assert_eq!(projection["type"], "globe", "{projection}");
}

/// The fixture of the expired sprite test for the file `file`: its style,
/// source and sprite sheet's JSON, or the offline test's tile and sprite
/// image.
fn online_fixture(file: &str) -> Response {
    match file {
        "sprite.png" | "0-0-0.vector.pbf" => file_response(&format!("test/fixtures/map/offline/{file}")),
        _ => file_response(&format!("test/fixtures/map/online/{file}")),
    }
}

/// A sprite sheet that expired in the cache loads again once the network
/// answers, after the map has loaded from the cache, and its images stay in
/// the style.
#[test]
fn expired_sprite_sheet() {
    let style_url = "http://127.0.0.1:3000/online/style.json";
    let dir = TempDir::new();
    {
        // The cache gets fresh copies of the style, its source and tile, and
        // expired ones of the sprite sheet.
        let seeding = StillMap::on(cached_runtime(&dir), StillOptions::default());
        serve(&seeding.runtime, |resource| {
            let response = online_fixture(file_name(&resource.url));
            if matches!(resource.kind, Kind::SpriteJSON | Kind::SpriteImage) { expired(response) } else { response }
        });
        seeding.load_url(style_url);
        seeding.render();
    }

    let map = StillMap::on(cached_runtime(&dir), StillOptions { mode: MapMode::Continuous, ..StillOptions::default() });
    let sprites_served = Arc::new(Mutex::new(BTreeSet::new()));
    let gate = Gate::install(&map.runtime, {
        let sprites_served = Arc::clone(&sprites_served);
        move |request: &ResourceRequest| {
            let resource = request.resource();
            let file = file_name(&resource.url).to_owned();
            let response = online_fixture(&file);
            if matches!(resource.kind, Kind::SpriteJSON | Kind::SpriteImage) {
                sprites_served.lock().insert(file);
            }
            response
        }
    });
    map.load_url(style_url);
    map.settle();
    assert!(sprites_served.lock().is_empty());

    // The network answers once the map has loaded from the cache, as it
    // would coming online then.
    gate.open();
    map.settle();
    assert_eq!(BTreeSet::from(["sprite.json".to_owned(), "sprite.png".to_owned()]), *sprites_served.lock());
    for image in ["a", "b", "c"] {
        assert!(wait(map.map.style_image(image)).is_some(), "{image}");
    }
}

/// A layer's properties read back as the style set them: constants,
/// transitions and expressions, and nothing for those it leaves unset.
#[test]
fn universal_style_getter() {
    let runtime = crate::support::runtime();
    let map = map(&runtime);
    wait(map.set_style_json(
        r#"{
        "version": 8,
        "sources": {
            "mapbox": {
                "type": "vector",
                "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]
            }
        },
        "layers": [{
            "id": "line",
            "type": "line",
            "source": "mapbox",
            "paint": {
                "line-color": "red",
                "line-opacity": 0.5,
                "line-width": ["get", "width"],
                "line-opacity-transition": {
                    "duration": 400,
                    "delay": 500
                }
            },
            "layout": {
                "line-cap": "butt"
            }
        }]
        }"#,
    ));
    let property = |name: &str| {
        wait(map.style_layer_property("line", name)).map(|json| serde_json::from_str::<Value>(&json).unwrap())
    };

    assert_eq!(property("nonexistent"), None);
    assert_eq!(property("line-blur"), None);
    assert_eq!(property("line-color-transition"), None);

    let color = property("line-color").expect("line-color");
    let color = color.as_array().expect("an rgba array");
    assert_eq!(color[0], "rgba");
    let channels: Vec<f64> = color[1..].iter().map(|channel| channel.as_f64().unwrap()).collect();
    assert_eq!(channels, [255.0, 0.0, 0.0, 1.0]);

    assert_eq!(property("line-opacity").and_then(|opacity| opacity.as_f64()), Some(0.5));

    let transition = property("line-opacity-transition").expect("line-opacity-transition");
    assert_eq!(transition.as_object().map(serde_json::Map::len), Some(2), "{transition}");

    assert_eq!(property("line-width"), Some(json!(["number", ["get", "width"]])));
    assert_eq!(property("line-cap"), Some(json!("butt")));
}

/// Switching styles drops the glyphs of the font stacks the new style no
/// longer uses, so switching back requests them again, and keeps the glyphs
/// of those it still uses.
#[test]
fn style_switch_evicts_the_glyphs_of_unused_font_stacks() {
    let map = StillMap::still();
    let requests = serve(&map.runtime, |resource| match resource.kind {
        Kind::Glyphs => file_response("test/fixtures/resources/glyphs.pbf"),
        _ => not_found(),
    });
    let style = |fonts: &[&str]| {
        let layers: Vec<String> = fonts
            .iter()
            .map(|font| {
                format!(
                    r#"{{"id": "{font}", "type": "symbol", "source": "point",
                        "layout": {{"text-field": "a", "text-font": ["{font}"]}}}}"#
                )
            })
            .collect();
        format!(
            r#"{{
                "version": 8,
                "glyphs": "test://glyphs/{{fontstack}}/{{range}}.pbf",
                "sources": {{
                    "point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}}
                }},
                "layers": [{}]
            }}"#,
            layers.join(", ")
        )
    };

    map.load_json(&style(&["Kept", "Dropped"]));
    map.render();
    assert_eq!(requests.count(Kind::Glyphs), 2, "{:?}", requests.all());
    map.load_json(&style(&["Kept"]));
    map.render();
    requests.clear();

    map.load_json(&style(&["Kept", "Dropped"]));
    map.render();
    assert_eq!(requests.urls(Kind::Glyphs), ["test://glyphs/Dropped/0-255.pbf"]);
}
