//! A crate outside the workspace's engine derives the worker codec through
//! the facade's re-export, as a layer type's bucket does.

use mln_rs::mln_wire::{Reader, Wire, Writer};

#[derive(Debug, PartialEq, Wire)]
#[wire(crate = mln_rs::mln_wire)]
struct Dots {
    positions: Vec<[i16; 2]>,
    radius: Option<f32>,
}

#[test]
fn a_type_derives_wire_through_the_facade_and_crosses_the_codec() {
    let dots = Dots { positions: vec![[1, -2], [300, 4]], radius: Some(2.5) };
    let mut writer = Writer::new();
    dots.write(&mut writer);
    let bytes = writer.finish().expect("bytes");
    assert_eq!(Dots::read(&mut Reader::new(&bytes)), Some(dots));
}
