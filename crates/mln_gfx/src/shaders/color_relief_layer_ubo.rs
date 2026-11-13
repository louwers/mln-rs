use bytemuck::{Pod, Zeroable};

/// The color relief shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ColorReliefDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<ColorReliefDrawableUBO>() == 64);

/// The color relief shader's per-tile uniform block (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ColorReliefTilePropsUBO {
    /*  0 */ pub unpack: [f32; 4], // DEM unpack vector
    /* 16 */ pub dimension: [f32; 2], // Texture dimensions
    /* 24 */ pub color_ramp_size: i32, // Number of color stops
    /* 28 */ pub pad_tile0: f32, // Padding for alignment
                        /* 32 */
}
const _: () = assert!(std::mem::size_of::<ColorReliefTilePropsUBO>() == 32);

/// The color relief shader's evaluated properties uniform block (16 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ColorReliefEvaluatedPropsUBO {
    /*  0 */ pub opacity: f32,
    /*  4 */ pub pad_eval0: f32,
    /*  8 */ pub pad_eval1: f32,
    /* 12 */ pub pad_eval2: f32,
    /* 16 */
}
const _: () = assert!(std::mem::size_of::<ColorReliefEvaluatedPropsUBO>() == 16);
