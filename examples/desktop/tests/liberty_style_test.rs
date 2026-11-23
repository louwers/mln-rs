// Loads OpenFreeMap Liberty (main.cpp's default style) over HTTPS through
// the path mln-desktop uses: a runtime's map, whose resource loader goes
// through the online file source to the platform's HTTP transport, and the
// style's parser. Then the projections the controls offer. Needs network
// access.

use mln_runtime::{Completion, MapOptions, Runtime, RuntimeOptions};
use mln_util::size::Size;

// The app's styles and projections (src/styles.rs).
#[path = "../src/styles.rs"]
#[expect(dead_code, reason = "the tests use only the projections")]
mod styles;

const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

fn map() -> (Runtime, mln_runtime::Map) {
    let runtime = mln_rs::start_runtime(&RuntimeOptions::default()).unwrap();
    let options = MapOptions {
        map: mln_runtime::map::MapCoreOptions { size: Size::new(256, 256), pixel_ratio: 1.0, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    (runtime, map)
}

#[test]
#[ignore = "needs network access"]
fn loads_liberty_over_https() {
    let (_runtime, map) = map();

    // Completes once the style has loaded and parsed, or failed to.
    map.set_style_url(LIBERTY).unwrap().wait().unwrap();

    assert_eq!(Some(LIBERTY.to_owned()), map.style_url().and_then(Completion::wait).unwrap());
    let json: serde_json::Value =
        serde_json::from_str(&map.loaded_style_json().and_then(Completion::wait).unwrap()).unwrap();
    assert_eq!(json["sources"]["openmaptiles"]["type"], "vector");
    let layers = map.style_layers().and_then(Completion::wait).unwrap();
    assert!(layers.len() > 100, "{} layers", layers.len());
    assert_eq!("background", layers[0].id);
    let sources = map.style_sources().and_then(Completion::wait).unwrap();
    assert!(sources.iter().any(|source| source.id == "openmaptiles"));
    assert!(map.snapshot().unwrap().update_generation > 0);
}

/// Each projection of the controls sets the style's, and `Style` puts back
/// the style's own: none, in Liberty.
#[test]
#[ignore = "needs network access"]
fn projections_override_the_style_and_style_restores_it() {
    let (_runtime, map) = map();
    map.set_style_url(LIBERTY).unwrap().wait().unwrap();
    let projection = |picked: styles::Projection| {
        map.with_core(move |map| picked.apply(map)).unwrap().wait().unwrap().unwrap();
        map.style_root_property("projection").and_then(Completion::wait).unwrap()
    };
    let own = map.style_root_property("projection").and_then(Completion::wait).unwrap();
    for picked in styles::Projection::ALL {
        let kind = match picked {
            styles::Projection::Style => continue,
            styles::Projection::Flat => "mercator",
            styles::Projection::Globe => "vertical-perspective",
            styles::Projection::Handoff => "globe",
        };
        let set: serde_json::Value = serde_json::from_str(&projection(picked).unwrap()).unwrap();
        assert_eq!(set["type"], kind, "{}: {}", picked.name(), picked.hint());
    }
    assert_eq!(own, projection(styles::Projection::Style));
}
