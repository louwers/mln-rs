//! A map whose registry lacks the crate still draws a style written for it.

use std::sync::Once;

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_headless::HeadlessFrontend;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;
use parking_lot::Mutex;

/// The messages of the warnings logged so far.
fn warnings() -> Vec<String> {
    static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Warnings;
    impl log::Log for Warnings {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                WARNINGS.lock().push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Warnings).expect("the test has no other logger");
        log::set_max_level(log::LevelFilter::Warn);
    });
    WARNINGS.lock().clone()
}

/// The image of `style`, drawn with the built-in types alone.
fn render(style: &str) -> PremultipliedImage {
    let size = Size::new(96, 64);
    let (queue, pool) = (Queue::<()>::new(), Pool::new(2));
    let mut frontend = HeadlessFrontend::new(size, 1.0, &queue, &pool).expect("a headless frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::new(options, mln_builtins::registry(), &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(style);
    frontend.render(&mut map).expect("a still image").image
}

const SHAPES: &str = r#""shapes":{"type":"geojson","data":{"type":"FeatureCollection","features":[
    {"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[-30,-30],[30,-30],[30,30],[-30,30],[-30,-30]]]}},
    {"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-50,-40],[50,40]]}}]}}"#;

#[test]
fn a_registry_without_the_crate_leaves_out_its_layers_and_draws_routed_layers_on_the_map() {
    warnings();
    let background = r##"{"id":"background","type":"background","paint":{"background-color":"#ffffff"}}"##;
    let fill = r##"{"id":"fill","type":"fill","source":"shapes","filter":["==",["geometry-type"],"Polygon"],
        "paint":{"fill-color":"#3080c0"}}"##;
    let line = r##"{"id":"line","type":"line","source":"shapes","filter":["==",["geometry-type"],"LineString"],
        "paint":{"line-color":"#c03020","line-width":4}}"##;
    let style = |root: &str, layers: &[&str]| {
        format!(r#"{{"version":8,"zoom":0.5,{root}"sources":{{{SHAPES}}},"layers":[{}]}}"#, layers.join(","))
    };

    let routed = fill.replacen('{', r#"{"org.maplibre:destination-target":"t","#, 1);
    let process = r#"{"id":"glow","type":"org.maplibre.image-process","org.maplibre:source-target":"t",
        "org.maplibre:operations":[{"type":"blur","size":4}]}"#;
    let image = render(&style(r#""org.maplibre:targets":[{"id":"t"}],"#, &[background, &routed, process, line]));
    let expected = render(&style("", &[background, fill, line]));
    assert!(image.data != render(&style("", &[background, line])).data, "the routed fill draws");
    assert!(image.data == expected.data, "the image differs from the style's without the crate's layers");

    let warnings = warnings();
    for says in ["org.maplibre.image-process", "org.maplibre:targets", "org.maplibre:destination-target"] {
        assert!(warnings.iter().any(|warning| warning.contains(says)), "no warning of {says} in {warnings:#?}");
    }
}
