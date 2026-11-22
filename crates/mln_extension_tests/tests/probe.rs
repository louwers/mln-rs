//! The `probe` layer type through the facade: the frame lends a layer
//! without a source the backend's commands before its passes and at the
//! layer's place, the layer's work after the GPU runs once it drops, and a
//! query hits what it draws without tile features.

mod support;

use mln_extension_tests::probe::{HIT_RADIUS, Probe, ProbeEnds, ProbeLayer};
use mln_rs::feature::Identifier;
use mln_rs::layer::{GpuEnd, LendAt};
use mln_rs::mln_render::RegistryBuilder;
use mln_rs::mln_util::geo::ScreenCoordinate;
use mln_rs::{BackendType, RenderedQueryGeometry, RenderedQueryOptions};
use support::{Fixture, SIZE, wait};

const STYLE: &str = r##"{"version":8,"sources":{},"layers":[
    {"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
    {"id":"probe","type":"probe"}]}"##;

#[test]
fn a_layer_draws_with_the_commands_lent_it_and_ends_after_the_gpu() {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<ProbeLayer>().unwrap();
    let fixture = Fixture::new(BackendType::WebGPU, builder.build(), STYLE);
    let ends = ProbeEnds::default();
    let given = ends.clone();
    wait(fixture.map.edit_style_layer_data::<Probe>("probe", move |data| {
        *data = given;
        Ok(())
    }));
    fixture.render();
    let lent =
        wait(fixture.session.query_rendered_layer::<ProbeLayer, _>("probe", |layer, _| Ok(layer.lent().to_vec())));
    let [(LendAt::BeforePasses, None), (LendAt::Pass, Some(depth))] = lent[..] else { panic!("lent {lent:?}") };
    assert!(depth > 0.0 && depth <= 1.0, "{depth}");

    wait(fixture.map.remove_style_layer("probe"));
    fixture.render();
    assert_eq!((1, 0), (ends.count(GpuEnd::Finished), ends.count(GpuEnd::Lost)));
}

#[test]
fn a_query_hits_what_a_layer_draws_without_tile_features() {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<ProbeLayer>().unwrap();
    let fixture = Fixture::new(BackendType::WebGPU, builder.build(), STYLE);
    fixture.render();
    let center = ScreenCoordinate::new(f64::from(SIZE.width) / 2.0, f64::from(SIZE.height) / 2.0);
    let hits = |x: f64| {
        let point = RenderedQueryGeometry::Point(ScreenCoordinate::new(center.x + x, center.y));
        let features = wait(fixture.session.query_rendered_features(point, RenderedQueryOptions::default()));
        features.into_iter().map(|feature| (feature.layer, feature.id)).collect::<Vec<_>>()
    };
    assert_eq!(vec![("probe".to_owned(), Identifier::Uint(1))], hits(HIT_RADIUS));
    assert_eq!(Vec::<(String, Identifier)>::new(), hits(HIT_RADIUS + 1.0));
}
