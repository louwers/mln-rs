use bytemuck::{Pod, Zeroable};

/// The layer composite shader's uniform block (16 bytes): the opacity the
/// layer's texture is composited with, and the blend the shader computes
/// against the backdrop texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LayerCompositeUBO {
    /*  0 */ pub opacity: f32,
    /// One of the `LayerCompositeBlendMode` values.
    /*  4 */
    pub blend: i32,
    /*  8 */ pub pad2: f32,
    /* 12 */ pub pad3: f32,
    /* 16 */
}
const _: () = assert!(size_of::<LayerCompositeUBO>() == 16);

/// How the layer composite shader blends the texture with the backdrop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LayerCompositeBlendMode {
    /// The texture is drawn as it is, for the pipeline's blend to composite.
    None = 0,
    // The shader composites the texture over the backdrop with these blend modes
    // of the W3C compositing spec, which no blend function expresses.
    Multiply = 1,
    Overlay = 2,
    Color = 3,
    Luminosity = 4,
}
