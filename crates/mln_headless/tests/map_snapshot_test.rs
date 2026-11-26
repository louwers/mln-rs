//! What rendering changes in `Map::snapshot`.

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_headless::HeadlessFrontend;
use mln_style::size::Size;

#[test]
fn a_rendered_still_shows_in_the_snapshot() {
    let run_loop = Queue::new();
    let pool = Pool::new(4);
    let size = Size::new(64, 64);
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop, &pool).expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }, &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(
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
