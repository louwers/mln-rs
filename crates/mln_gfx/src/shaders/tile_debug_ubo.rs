//! The uniform blocks of the tile debug overlays' shader
//! ([`TILE_DEBUG`](crate::shaders::programs::TILE_DEBUG)).

use bytemuck::{Pod, Zeroable};

/// A draw's uniform block (96 bytes): the matrix of its tile, and the ratio
/// of its tile units to pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TileDebugDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub ratio: f32,
    /* 68 */ pub pad: [f32; 7],
    /* 96 */
}
const _: () = assert!(size_of::<TileDebugDrawableUBO>() == 6 * 16);

/// A draw's element of an overlay's buffer of [`TileDebugDrawableUBO`]s
/// (128 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TileDebugDrawableElement {
    /*  0 */ pub drawable: TileDebugDrawableUBO,
    /* 96 */ pub pad: [f32; 8],
    /* 128 */
}
const _: () = assert!(size_of::<TileDebugDrawableElement>() == 8 * 16);

/// An overlay's line properties (48 bytes). `color` is premultiplied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TileDebugPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub blur: f32,
    /* 20 */ pub opacity: f32,
    /* 24 */ pub gapwidth: f32,
    /* 28 */ pub offset: f32,
    /* 32 */ pub width: f32,
    /* 36 */ pub pad: [f32; 3],
    /* 48 */
}
const _: () = assert!(size_of::<TileDebugPropsUBO>() == 3 * 16);
