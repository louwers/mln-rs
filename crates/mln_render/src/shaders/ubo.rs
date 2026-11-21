//! The uniform blocks of the engine's programs.

mln_gfx::uniform_block! {
    /// The sky properties uniform block (64 bytes). The colors are premultiplied
    /// r, g, b, a floats.
    pub(crate) struct SkyPropsUBO {
        pub sky_color: [f32; 4],
        pub horizon_color: [f32; 4],
        pub horizon: [f32; 2],
        pub horizon_normal: [f32; 2],
        pub viewport_size: [f32; 2],
        pub sky_horizon_blend: f32,
        pub sky_blend: f32,
    }
}
const _: () = assert!(size_of::<SkyPropsUBO>() == 4 * 16);

mln_gfx::uniform_block! {
    /// The atmosphere properties uniform block (96 bytes).
    pub(crate) struct AtmospherePropsUBO {
        pub inv_view_projection: [f32; 4 * 4],
        pub camera_position: [f32; 4],
        /// The fourth component stores the evaluated atmosphere blend.
        pub sun_position: [f32; 4],
    }
}
const _: () = assert!(size_of::<AtmospherePropsUBO>() == 6 * 16);

mln_gfx::uniform_block! {
    /// An overlay's color, premultiplied, the width of its segments in pixels,
    /// and the width of the world in pixels at the frame's zoom, which sets the
    /// tile units a pixel spans (32 bytes).
    pub(crate) struct TileDebugPropsUBO {
        pub color: [f32; 4],
        pub width: f32,
        pub world_width: f32,
        pub pad: [f32; 2],
    }
}
const _: () = assert!(size_of::<TileDebugPropsUBO>() == 2 * 16);

mln_gfx::uniform_block! {
    /// The layer composite shader's uniform block (16 bytes): the opacity the
    /// layer's texture is composited with, and the blend the shader computes
    /// against the backdrop texture.
    pub(crate) struct LayerCompositeUBO {
        pub opacity: f32,
        /// One of the `LAYER_COMPOSITE_BLEND_*` constants.
        pub blend: i32,
        pub pad2: f32,
        pub pad3: f32,
    }
}
const _: () = assert!(size_of::<LayerCompositeUBO>() == 16);

/// The texture is drawn as it is, for the pipeline's blend to composite.
pub(crate) const LAYER_COMPOSITE_BLEND_NONE: i32 = 0;
// The shader composites the texture over the backdrop with these blend modes
// of the W3C compositing spec, which no blend function expresses.
pub(crate) const LAYER_COMPOSITE_BLEND_MULTIPLY: i32 = 1;
pub(crate) const LAYER_COMPOSITE_BLEND_OVERLAY: i32 = 2;
pub(crate) const LAYER_COMPOSITE_BLEND_COLOR: i32 = 3;
pub(crate) const LAYER_COMPOSITE_BLEND_LUMINOSITY: i32 = 4;
