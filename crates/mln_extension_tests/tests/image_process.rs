//! The `mln_image_process` crate's keys and the engine's keys of named
//! targets through the facade: a map reads, writes and refuses their JSON,
//! and a runtime whose registry has only the built-ins loads a style of them
//! with a warning for the image-process layer type, and draws the layers
//! that the style routes into targets into them.

mod support;

use std::sync::Once;

use mln_rs::mln_render::RegistryBuilder;
use mln_rs::mln_util::ErrorKind;
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::mln_util::logging::{Event, EventSeverity, Log, Observer};
use parking_lot::Mutex;
use support::{Fixture, wait};

/// A square and a line, drawn into the target `t` and blurred onto the map
/// by an image-process layer where `keys`, and otherwise drawn on the map.
fn style(keys: bool) -> String {
    let (targets, into, blur) = if keys {
        (
            r#""org.maplibre:targets":[{"id":"t"}],"#,
            r#""org.maplibre:destination-target":"t","#,
            r#",{"id":"blur","type":"org.maplibre.image-process","org.maplibre:source-target":"t",
                "org.maplibre:operations":[{"type":"blur","size":2}]}"#,
        )
    } else {
        ("", "", "")
    };
    format!(
        r##"{{"version":8,"zoom":2,"center":[20,20],"transition":{{"duration":0}},{targets}
        "sources":{{"shapes":{{"type":"geojson","data":{{"type":"FeatureCollection","features":[
            {{"type":"Feature","properties":{{}},"geometry":{{"type":"Polygon","coordinates":[[[15,15],[22,15],[22,22],[15,22],[15,15]]]}}}},
            {{"type":"Feature","properties":{{}},"geometry":{{"type":"LineString","coordinates":[[12,25],[28,14]]}}}}]}}}}}},
        "layers":[
            {{"id":"background","type":"background","paint":{{"background-color":"#ffffff"}}}},
            {{"id":"fill","type":"fill","source":"shapes",{into}
                "filter":["==",["geometry-type"],"Polygon"],"paint":{{"fill-color":"#3070c0"}}}},
            {{"id":"line","type":"line","source":"shapes",{into}
                "filter":["==",["geometry-type"],"LineString"],"paint":{{"line-color":"#c03020","line-width":3}}}}{blur}]}}"##
    )
}

/// The warnings logged from the first call on.
fn warnings() -> Vec<String> {
    static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Warnings;
    impl Observer for Warnings {
        fn on_record(&self, severity: EventSeverity, _: Event, _: i64, msg: &str) -> bool {
            if severity == EventSeverity::Warning {
                WARNINGS.lock().push(msg.to_owned());
            }
            false
        }
    }
    static OBSERVE: Once = Once::new();
    OBSERVE.call_once(|| Log::set_observer(Box::new(Warnings)));
    WARNINGS.lock().clone()
}

/// The message of the error that `result` completes with, which says the
/// argument is invalid.
fn refused<T: Send + 'static>(result: Result<mln_rs::Completion<T>, mln_rs::Error>) -> String {
    let error = result.unwrap().wait().err().expect("an error");
    assert_eq!(ErrorKind::InvalidArgument, error.kind, "{}", error.message);
    error.message
}

#[test]
fn a_map_reads_writes_and_refuses_the_keys_json() {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_image_process::register(&mut builder).unwrap();
    let fixture = Fixture::new(builder.build(), &style(true));
    let map = &fixture.map;
    let read = |name: &str| wait(map.style_layer_property("blur", name));
    assert_eq!(Some(r#"[{"size":2.0,"type":"blur"}]"#.to_owned()), read("org.maplibre:operations"));
    assert_eq!(Some(r#""t""#.to_owned()), read("org.maplibre:source-target"));

    let operations = r##"[{"type":"blur","size":["interpolate",["linear"],["zoom"],0,1,10,4]},
        {"type":"threshold"},{"type":"color","value":"#ff0000"},{"type":"opacity","value":0.5},
        {"type":"combine","target":"t","mode":"under"}]"##;
    wait(map.set_style_layer_property("blur", "org.maplibre:operations", operations));
    let written = r#"[{"size":["interpolate",["linear"],["zoom"],0.0,1.0,10.0,4.0],"type":"blur"},"#.to_owned()
        + r#"{"cutoff":128.0,"type":"threshold"},{"type":"color","value":["rgba",255.0,0.0,0.0,1.0]},"#
        + r#"{"type":"opacity","value":0.5},{"mode":"under","target":"t","type":"combine"}]"#;
    assert_eq!(Some(written), read("org.maplibre:operations"));

    wait(map.set_style_root_property("org.maplibre:targets", r#"[{"id":"t"},{"id":"u","precision":"high"}]"#));
    let targets = wait(map.style_root_property("org.maplibre:targets"));
    assert_eq!(Some(r#"[{"id":"t"},{"id":"u","precision":"high"}]"#.to_owned()), targets);

    for operations in [
        r#"[{"type":"invert"}]"#,
        r#"[{"type":"blur"}]"#,
        r#"[{"type":"blur","size":null}]"#,
        r#"[{"type":"color","value":null}]"#,
        r#"[{"type":"blur","size":2,"sigma":2}]"#,
        r#"[{"type":"combine","target":"t","mode":"screen"}]"#,
        r#"[{"type":"opacity","value":["get","opacity"]}]"#,
        r#"{"type":"blur","size":2}"#,
    ] {
        refused(map.set_style_layer_property("blur", "org.maplibre:operations", operations));
    }
    let message = refused(map.set_style_root_property("org.maplibre:targets", r#"[{"id":"t"},{"id":"t"}]"#));
    assert!(message.contains("a second target \"t\""), "{message}");
    refused(map.set_style_root_property("org.maplibre:targets", r#"[{"id":"t","precision":"low"}]"#));
    refused(map.set_style_layer_property("fill", "org.maplibre:source-target", r#""t""#));
}

fn render(style: &str) -> PremultipliedImage {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    let fixture = Fixture::new(builder.build(), style);
    wait(fixture.session.read_texture())
}

#[test]
fn a_registry_without_the_crate_leaves_out_its_layer_and_draws_into_the_targets() {
    warnings();
    let image = render(&style(true));
    let warnings = warnings();
    assert!(warnings.iter().any(|warning| warning.contains("org.maplibre.image-process")), "{warnings:#?}");
    for name in ["org.maplibre:targets", "org.maplibre:destination-target"] {
        assert!(!warnings.iter().any(|warning| warning.contains(name)), "a warning of {name} in {warnings:#?}");
    }

    // The square and the line draw into the target, which nothing reads.
    assert!(image.data.chunks(4).all(|pixel| pixel == [0xff; 4]), "the map isn't all white");
    let drawn = render(&style(false));
    for color in [[0x30, 0x70, 0xc0, 0xff], [0xc0, 0x30, 0x20, 0xff]] {
        assert!(drawn.data.chunks(4).any(|pixel| pixel == color), "no pixel of {color:02x?} on the map");
    }
}
