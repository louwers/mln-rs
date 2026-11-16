use bytemuck::{Pod, Zeroable};

/// The layer composite shader's uniform block (16 bytes): the opacity the
/// layer's texture is composited with, and the blend the shader computes
/// against the backdrop texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LayerCompositeUBO {
    /*  0 */ pub opacity: f32,
    /// One of the `LAYER_COMPOSITE_BLEND_*` constants.
    /*  4 */
    pub blend: i32,
    /*  8 */ pub pad2: f32,
    /* 12 */ pub pad3: f32,
    /* 16 */
}
const _: () = assert!(size_of::<LayerCompositeUBO>() == 16);

/// The texture is drawn as it is, for the pipeline's blend to composite.
pub const LAYER_COMPOSITE_BLEND_NONE: i32 = 0;
/// The shader composites the texture over the backdrop with each of the
/// W3C compositing spec's blend modes that no blend function expresses.
pub const LAYER_COMPOSITE_BLEND_MULTIPLY: i32 = 1;
pub const LAYER_COMPOSITE_BLEND_OVERLAY: i32 = 2;
pub const LAYER_COMPOSITE_BLEND_COLOR: i32 = 3;
pub const LAYER_COMPOSITE_BLEND_LUMINOSITY: i32 = 4;
