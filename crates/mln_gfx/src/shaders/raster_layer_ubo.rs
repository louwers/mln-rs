use bytemuck::{Pod, Zeroable};

/// The raster shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct RasterDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<RasterDrawableUBO>() == 4 * 16);

/// Evaluated properties that do not depend on the tile (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct RasterEvaluatedPropsUBO {
    /*  0 */ pub spin_weights: [f32; 4],
    /* 16 */ pub tl_parent: [f32; 2],
    /* 24 */ pub scale_parent: f32,
    /* 28 */ pub buffer_scale: f32,
    /* 32 */ pub fade_t: f32,
    /* 36 */ pub opacity: f32,
    /* 40 */ pub brightness_low: f32,
    /* 44 */ pub brightness_high: f32,
    /* 48 */ pub saturation_factor: f32,
    /* 52 */ pub contrast_factor: f32,
    /* 56 */ pub pad1: f32,
    /* 60 */ pub pad2: f32,
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<RasterEvaluatedPropsUBO>() == 4 * 16);
