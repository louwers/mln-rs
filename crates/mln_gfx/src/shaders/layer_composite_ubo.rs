use bytemuck::{Pod, Zeroable};

/// The layer composite shader's uniform block (16 bytes): the opacity the
/// layer's texture is composited with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LayerCompositeUBO {
    /*  0 */ pub opacity: f32,
    /*  4 */ pub pad1: f32,
    /*  8 */ pub pad2: f32,
    /* 12 */ pub pad3: f32,
    /* 16 */
}
const _: () = assert!(size_of::<LayerCompositeUBO>() == 16);
