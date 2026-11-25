//! The blocks of the sky, the atmosphere and the layer composite.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The sky properties uniform block. The colors are premultiplied
    /// r, g, b, a floats.
    pub struct SkyPropsUBO {
        pub sky_color: [f32; 4],
        pub horizon_color: [f32; 4],
        pub horizon: [f32; 2],
        pub horizon_normal: [f32; 2],
        pub viewport_size: [f32; 2],
        pub sky_horizon_blend: f32,
        pub sky_blend: f32,
    }
}

uniform_block! {
    /// The atmosphere properties uniform block.
    pub struct AtmospherePropsUBO {
        pub inv_view_projection: [f32; 4 * 4],
        pub camera_position: [f32; 4],
        /// The fourth component stores the evaluated atmosphere blend.
        pub sun_position: [f32; 4],
    }
}

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
