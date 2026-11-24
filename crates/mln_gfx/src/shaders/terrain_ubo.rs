use bytemuck::{Pod, Zeroable};

/// The terrain's uniform block (32 bytes): how its DEM's texels decode to
/// heights, as [`mln_util::dem::DemUnpack`] does, and the exaggeration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainPropsUBO {
    /// The weights of a texel's red, green and blue channels, from 0 to
    /// 255, then the base added to their sum.
    /*  0 */
    pub unpack: [f32; 4],
    /// What the sum is multiplied by, to meters.
    /* 16 */
    pub unpack_scale: f32,
    /* 20 */ pub exaggeration: f32,
    /* 24 */ pub pad0: f32,
    /* 28 */ pub pad1: f32,
    /* 32 */
}
const _: () = assert!(size_of::<TerrainPropsUBO>() == 2 * 16);
