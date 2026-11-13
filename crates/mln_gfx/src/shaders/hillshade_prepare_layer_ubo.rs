use bytemuck::{Pod, Zeroable};

/// The hillshade prepare shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadePrepareDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(size_of::<HillshadePrepareDrawableUBO>() == 4 * 16);

/// The hillshade prepare shader's per-tile uniform block (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadePrepareTilePropsUBO {
    /*  0 */ pub unpack: [f32; 4],
    /* 16 */ pub dimension: [f32; 2],
    /* 24 */ pub zoom: f32,
    /* 28 */ pub maxzoom: f32,
    /* 32 */
}
const _: () = assert!(size_of::<HillshadePrepareTilePropsUBO>() == 2 * 16);
