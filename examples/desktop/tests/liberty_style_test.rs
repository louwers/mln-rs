// Loads OpenFreeMap Liberty (main.cpp's default style) over HTTPS through
// the path mln-desktop uses: Map -> its ResourceLoader ->
// OnlineFileSource -> HTTPFileSource (the platform transport),
// `Style::loadURL`, the Map's style-response event and the parser. Needs
// network access.

use std::time::{Duration, Instant};

use mln_actor::{RunLoop, Timer};
use mln_map::{Map, MapEvent, MapOptions};
use mln_util::size::Size;

const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

#[test]
#[ignore = "needs network access"]
fn loads_liberty_over_https() {
    let run_loop = RunLoop::new();
    let mut map = Map::new(MapOptions { size: Size::new(256, 256), pixel_ratio: 1.0, ..Default::default() }).unwrap();

    map.style_mut().load_url(LIBERTY);
    assert!(!map.style().is_loaded());

    // Give up after 60 s: the timer wakes the loop then.
    let timeout = Timer::new();
    timeout.start(Duration::from_secs(60), Duration::ZERO, || {});
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    let mut loaded = false;
    let mut failed = None;
    while !loaded && failed.is_none() && Instant::now() < deadline {
        if !run_loop.run_once() {
            run_loop.wait_for_work();
        }
        for event in map.drain_events() {
            match event {
                MapEvent::StyleLoaded => loaded = true,
                MapEvent::LoadingFailed(error) => failed = Some(error.to_string()),
                _ => {}
            }
        }
    }

    assert_eq!(None, failed);
    assert!(loaded, "the style did not load within 60 s");

    let style = map.style();
    assert_eq!(LIBERTY, style.get_url());
    let json: serde_json::Value = serde_json::from_str(&style.get_json()).unwrap();
    assert_eq!(json["sources"]["openmaptiles"]["type"], "vector");
    let layers = style.get_layers();
    assert!(layers.len() > 100, "{} layers", layers.len());
    assert_eq!("background", layers[0].get_id());
    assert!(style.get_sources().iter().any(|s| s.get_id() == "openmaptiles"));
    assert!(map.update_generation() > 0);
}
