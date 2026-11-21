//! The uniform block of the tile debug overlays' shader
//! ([`TILE_DEBUG`](crate::shaders::programs::TILE_DEBUG)).

use bytemuck::{Pod, Zeroable};

/// An overlay's color, premultiplied, the width of its segments in pixels,
/// and the width of the world in pixels at the frame's zoom, which sets the
/// tile units a pixel spans (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TileDebugPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub width: f32,
    /* 20 */ pub world_width: f32,
    /* 24 */ pub pad: [f32; 2],
    /* 32 */
}
const _: () = assert!(size_of::<TileDebugPropsUBO>() == 2 * 16);
