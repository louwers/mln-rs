//! The `probe` layer type through the facade: the frame lends a layer
//! without a source the backend's commands before its passes and at the
//! layer's place, and the layer's work after the GPU runs once it drops.

mod support;

use mln_extension_tests::probe::{Probe, ProbeEnds, ProbeLayer};
use mln_rs::layer::{GpuEnd, LendAt};
use mln_rs::mln_render::RegistryBuilder;
use support::{Fixture, wait};

const STYLE: &str = r##"{"version":8,"sources":{},"layers":[
    {"id":"background","type":"background","paint":{"background-color":"#ffffff"}},
    {"id":"probe","type":"probe"}]}"##;

#[test]
fn a_layer_draws_with_the_commands_lent_it_and_ends_after_the_gpu() {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    builder.add_layer_type::<ProbeLayer>().unwrap();
    let fixture = Fixture::new(builder.build(), STYLE);
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
