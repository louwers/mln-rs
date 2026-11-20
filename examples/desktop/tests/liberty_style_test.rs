// Loads OpenFreeMap Liberty (main.cpp's default style) over HTTPS through
// the path mln-desktop uses: a runtime's map, whose resource loader goes
// through the online file source to the platform's HTTP transport, and the
// style's parser. Needs network access.

use mln_runtime::{Completion, MapOptions, RuntimeOptions};
use mln_util::size::Size;

const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

#[test]
#[ignore = "needs network access"]
fn loads_liberty_over_https() {
    let runtime = mln_rs::start_runtime(&RuntimeOptions::default()).unwrap();
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), pixel_ratio: 1.0, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();

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
