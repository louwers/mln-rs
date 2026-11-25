use crate::shaders::block::uniform_block;

uniform_block! {
    /// The layer composite shader's uniform block: the opacity the
    /// layer's texture is composited with, and the blend the shader computes
    /// against the backdrop texture.
    pub struct LayerCompositeUBO {
        pub opacity: f32,
        /// One of the `LAYER_COMPOSITE_BLEND_*` constants.
        pub blend: i32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

/// The texture is drawn as it is, for the pipeline's blend to composite.
pub const LAYER_COMPOSITE_BLEND_NONE: i32 = 0;
// The shader composites the texture over the backdrop with these blend modes
// of the W3C compositing spec, which no blend function expresses.
pub const LAYER_COMPOSITE_BLEND_MULTIPLY: i32 = 1;
pub const LAYER_COMPOSITE_BLEND_OVERLAY: i32 = 2;
pub const LAYER_COMPOSITE_BLEND_COLOR: i32 = 3;
pub const LAYER_COMPOSITE_BLEND_LUMINOSITY: i32 = 4;
