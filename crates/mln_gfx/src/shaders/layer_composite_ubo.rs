use crate::shaders::block::uniform_block;

uniform_block! {
    /// The layer composite shader's uniform block: the opacity the
    /// layer's texture is composited with, and the blend the shader computes
    /// against the backdrop texture.
    pub struct LayerCompositeUBO {
        pub opacity: f32,
        /// One of the `LayerCompositeBlendMode` values.
        pub blend: i32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

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
