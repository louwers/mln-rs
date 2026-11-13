//! What rendering changes in `Map::snapshot`.

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_util::size::Size;

#[test]
fn a_rendered_still_shows_in_the_snapshot() {
    let _run_loop = RunLoop::new();
    let size = Size::new(64, 64);
    let mut frontend = HeadlessFrontend::new(size, 1.0).expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }).unwrap();
    frontend.attach(&map);
    map.style_mut().load_json(
        r#"{"version":8,"sources":{},"layers":[{"id":"background","type":"background","paint":{"background-color":"red"}}]}"#,
    );

    let before = map.snapshot();
    assert!(!before.fully_loaded);
    assert_eq!(before.update_generation, 0);

    frontend.render(&mut map).expect("render");

    let after = map.snapshot();
    assert!(after.fully_loaded);
    assert!(after.generation > before.generation);
    assert!(after.update_generation > before.update_generation);
}
