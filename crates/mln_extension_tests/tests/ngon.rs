//! The `org.maplibre.ngon` layer type through the facade, as a caller adds
//! it: a runtime whose registry has it draws a style with an n-gon layer,
//! and what edits of the layer and of its source make.

mod support;

use std::sync::Arc;

use mln_ngon::NgonLayer;
use mln_rs::mln_render::{Registry, RegistryBuilder};
use support::{Fixture, SIZE, wait};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

/// A red square of radius 8 at the center of a white map, its sides level.
const LAYER: &str = r##"{"id":"ngons","type":"org.maplibre.ngon","source":"points",
    "paint":{"ngon-color":"#ff0000","ngon-radius":8,"ngon-corners":4,"ngon-rotate":45}}"##;

fn style() -> String {
    format!(
        r##"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"points":{{"type":"geojson","data":{{"type":"Feature","properties":{{}},
            "geometry":{{"type":"Point","coordinates":[0,0]}}}}}}}},
        "layers":[{{"id":"background","type":"background","paint":{{"background-color":"#ffffff"}}}},{LAYER}]}}"##
    )
}

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_ngon::register(&mut builder).unwrap();
    builder.build()
}

/// The premultiplied RGBA of the pixel `x` pixels right of the center of
/// the last frame.
fn pixel(fixture: &Fixture, x: u32) -> [u8; 4] {
    let image = wait(fixture.session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2 + x) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn a_removed_layer_goes_and_a_new_source_draws_its_own_data() {
    let fixture = Fixture::new(registry(), &style());
    assert_eq!(RED, pixel(&fixture, 0));
    assert_eq!(RED, pixel(&fixture, 4));
    assert_eq!(WHITE, pixel(&fixture, 8));

    wait(fixture.map.remove_style_layer("ngons"));
    fixture.render();
    assert_eq!(WHITE, pixel(&fixture, 0));
    let error =
        fixture.session.query_rendered_layer::<NgonLayer, _>("ngons", |_, _| Ok(())).unwrap().wait().unwrap_err();
    assert_eq!(mln_rs::mln_util::ErrorKind::NotFound, error.kind);

    wait(fixture.map.add_style_layer_json(LAYER, None));
    fixture.render_loaded();
    assert_eq!(RED, pixel(&fixture, 0));

    // The layer's source changes to one whose only point is 14 pixels east
    // of the center, at zoom 0.
    let east = r#"{"type":"geojson","data":{"type":"Feature","properties":{},
        "geometry":{"type":"Point","coordinates":[9.84375,0]}}}"#;
    wait(fixture.map.add_style_source_json("east", east));
    wait(fixture.map.set_style_layer_property("ngons", "source", "\"east\""));
    fixture.render_loaded();
    assert_eq!(WHITE, pixel(&fixture, 0));
    assert_eq!(RED, pixel(&fixture, 14));
}
