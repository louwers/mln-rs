use bytemuck::{Pod, Zeroable};

/// The hillshade prepare shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadePrepareDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(size_of::<HillshadePrepareDrawableUBO>() == 4 * 16);

/// The hillshade prepare shader's per-tile uniform block (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadePrepareTilePropsUBO {
    /// How the DEM's texels decode, as [`mln_util::dem::DemUnpack`] does:
    /// the weights of a texel's red, green and blue channels, from 0 to 255,
    /// then the base added to their sum.
    /*  0 */
    pub unpack: [f32; 4],
    /* 16 */ pub dimension: [f32; 2],
    /* 24 */ pub zoom: f32,
    /* 28 */ pub maxzoom: f32,
    /// What the sum is multiplied by, to meters.
    /* 32 */
    pub unpack_scale: f32,
    /* 36 */ pub pad0: f32,
    /* 40 */ pub pad1: f32,
    /* 44 */ pub pad2: f32,
    /* 48 */
}
const _: () = assert!(size_of::<HillshadePrepareTilePropsUBO>() == 3 * 16);
