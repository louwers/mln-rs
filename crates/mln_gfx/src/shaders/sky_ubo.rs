// Ports include/mln/shaders/sky_ubo.hpp

use bytemuck::{Pod, Zeroable};

/// `struct alignas(16) SkyPropsUBO` (64 bytes). The colors are the
/// premultiplied `mln::Color` as r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct SkyPropsUBO {
    /*  0 */ pub sky_color: [f32; 4],
    /* 16 */ pub horizon_color: [f32; 4],
    /* 32 */ pub horizon: [f32; 2],
    /* 40 */ pub horizon_normal: [f32; 2],
    /* 48 */ pub viewport_size: [f32; 2],
    /* 56 */ pub sky_horizon_blend: f32,
    /* 60 */ pub sky_blend: f32,
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<SkyPropsUBO>() == 4 * 16);

/// `struct alignas(16) AtmospherePropsUBO` (96 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct AtmospherePropsUBO {
    /*  0 */ pub inv_view_projection: [f32; 4 * 4],
    /* 64 */ pub camera_position: [f32; 4],
    // The fourth component stores the evaluated atmosphere blend.
    /* 80 */
    pub sun_position: [f32; 4],
    /* 96 */
}
const _: () = assert!(std::mem::size_of::<AtmospherePropsUBO>() == 6 * 16);
