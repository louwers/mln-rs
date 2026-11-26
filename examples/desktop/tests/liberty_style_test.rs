// Loads OpenFreeMap Liberty (main.cpp's default style) over HTTPS through
// the path mln-desktop uses: a runtime's map, whose resource loader goes
// through the online file source to the platform's HTTP transport, and the
// style's parser. Then the projections the controls offer, and the
// picker's Satellite 3D and Liberty 3D, which add Mapterhorn's terrain to
// VersaTiles' Satellite and to Liberty. Needs network access.

use std::time::{Duration, Instant};

use mln_runtime::{Completion, MapOptions, Runtime, RuntimeOptions};
use mln_style::size::Size;

// The app's styles and the edits it makes to them (src/styles.rs).
#[path = "../src/styles.rs"]
mod styles;

const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

fn map() -> (Runtime, mln_runtime::Map) {
    let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
    let options = MapOptions {
        map: mln_engine::map::MapOptions { size: Size::new(256, 256), pixel_ratio: 1.0, ..Default::default() },
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

/// Satellite 3D: Satellite, then a Mapterhorn DEM and the style's terrain on
/// it, without a hillshade, and Mapterhorn's credit once its TileJSON loads.
#[test]
#[ignore = "needs network access"]
fn satellite_3d_adds_mapterhorn_terrain_to_satellite() {
    let satellite_3d = styles::style_for("satellite-3d");
    let styles::StyleSource::Url(url) = satellite_3d.source else { panic!("Satellite 3D has no URL") };
    // The URL opens the plain style.
    assert_eq!(None, styles::style_for(&url).terrain);
    let terrain = satellite_3d.terrain.unwrap();
    assert_eq!(None, terrain.hillshade);

    let (_runtime, map) = map();
    map.set_style_url(&url).unwrap().wait().unwrap();
    let layers = || -> Vec<String> {
        map.style_layers().and_then(Completion::wait).unwrap().into_iter().map(|layer| layer.id).collect()
    };
    let before = layers();
    for _ in 0..2 {
        // The second time changes nothing.
        let added = terrain.clone();
        map.with_core(move |map| styles::add_terrain(map, &added)).unwrap().wait().unwrap().unwrap();
    }

    let json = map.style_root_property("terrain").and_then(Completion::wait).unwrap().unwrap();
    let set: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(set["source"], styles::TERRAIN_SOURCE);
    assert!((1.0..=2.0).contains(&set["exaggeration"].as_f64().unwrap()), "{set}");
    assert_eq!(before, layers());

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let attribution = map
            .with_core(|map| map.source(styles::TERRAIN_SOURCE).and_then(|source| source.data().attribution()))
            .and_then(Completion::wait)
            .unwrap();
        if let Some(attribution) = attribution {
            assert!(attribution.contains("Mapterhorn"), "{attribution}");
            break;
        }
        assert!(Instant::now() < deadline, "Mapterhorn's TileJSON didn't load");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Liberty 3D: Liberty, then a Mapterhorn DEM, the style's terrain on it, a
/// hillshade of it right under Liberty's buildings, and a sky, which Liberty
/// lacks.
#[test]
#[ignore = "needs network access"]
fn liberty_3d_shades_the_ground_under_libertys_buildings() {
    let terrain = styles::style_for("liberty-3d").terrain.unwrap();
    let (_runtime, map) = map();
    map.set_style_url(LIBERTY).unwrap().wait().unwrap();
    let added = terrain.clone();
    map.with_core(move |map| styles::add_terrain(map, &added)).unwrap().wait().unwrap().unwrap();

    let layers: Vec<String> =
        map.style_layers().and_then(Completion::wait).unwrap().into_iter().map(|layer| layer.id).collect();
    let building = layers.iter().position(|id| id == "building").expect("Liberty's buildings");
    assert_eq!(layers[building - 1], "terrain-hillshade", "{layers:?}");
    let json = map.style_root_property("terrain").and_then(Completion::wait).unwrap().unwrap();
    let set: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(set["source"], styles::TERRAIN_SOURCE);
    // The catalog's sky: the properties it sets, whose colors the style
    // writes back as `rgba` expressions.
    let json = map.style_root_property("sky").and_then(Completion::wait).unwrap().expect("Liberty 3D's sky");
    let keys = |json: &str| {
        let sky: serde_json::Map<String, serde_json::Value> = serde_json::from_str(json).unwrap();
        sky.into_iter().map(|(key, _)| key).collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(keys(&json), keys(&terrain.sky.unwrap()), "{json}");
}
