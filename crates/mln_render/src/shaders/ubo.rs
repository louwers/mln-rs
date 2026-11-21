//! The uniform blocks of the engine's programs.

use bytemuck::{Pod, Zeroable};

/// The sky properties uniform block (64 bytes). The colors are premultiplied
/// r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub(crate) struct SkyPropsUBO {
    /*  0 */ pub sky_color: [f32; 4],
    /* 16 */ pub horizon_color: [f32; 4],
    /* 32 */ pub horizon: [f32; 2],
    /* 40 */ pub horizon_normal: [f32; 2],
    /* 48 */ pub viewport_size: [f32; 2],
    /* 56 */ pub sky_horizon_blend: f32,
    /* 60 */ pub sky_blend: f32,
    /* 64 */
}
const _: () = assert!(size_of::<SkyPropsUBO>() == 4 * 16);

/// The atmosphere properties uniform block (96 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub(crate) struct AtmospherePropsUBO {
    /*  0 */ pub inv_view_projection: [f32; 4 * 4],
    /* 64 */ pub camera_position: [f32; 4],
    /// The fourth component stores the evaluated atmosphere blend.
    /* 80 */
    pub sun_position: [f32; 4],
    /* 96 */
}
const _: () = assert!(size_of::<AtmospherePropsUBO>() == 6 * 16);

/// An overlay's color, premultiplied, the width of its segments in pixels,
/// and the width of the world in pixels at the frame's zoom, which sets the
/// tile units a pixel spans (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub(crate) struct TileDebugPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub width: f32,
    /* 20 */ pub world_width: f32,
    /* 24 */ pub pad: [f32; 2],
    /* 32 */
}
const _: () = assert!(size_of::<TileDebugPropsUBO>() == 2 * 16);

/// The layer composite shader's uniform block (16 bytes): the opacity the
/// layer's texture is composited with, and the blend the shader computes
/// against the backdrop texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub(crate) struct LayerCompositeUBO {
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
pub(crate) const LAYER_COMPOSITE_BLEND_NONE: i32 = 0;
// The shader composites the texture over the backdrop with these blend modes
// of the W3C compositing spec, which no blend function expresses.
pub(crate) const LAYER_COMPOSITE_BLEND_MULTIPLY: i32 = 1;
pub(crate) const LAYER_COMPOSITE_BLEND_OVERLAY: i32 = 2;
pub(crate) const LAYER_COMPOSITE_BLEND_COLOR: i32 = 3;
pub(crate) const LAYER_COMPOSITE_BLEND_LUMINOSITY: i32 = 4;
