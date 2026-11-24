use bytemuck::{Pod, Zeroable};

/// A terrain tile's draw block (16 bytes), an entry of the storage buffer
/// that the terrain's draws index by `globalIndex`. The tile projects
/// through its entry of
/// [`ID_PROJECTION_UBO`](crate::shaders::shader_defines::ID_PROJECTION_UBO).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainDrawableUBO {
    /// Where the tile reads its DEM: the scale and offset that map the
    /// tile's uv to the DEM's (`uv * scale + offset`), then the DEM's width
    /// in pixels. A width of 0 is flat ground.
    /* 0 */
    pub dem_coords: [f32; 4],
    /* 16 */
}
const _: () = assert!(size_of::<TerrainDrawableUBO>() == 16);

/// The terrain's uniform block (32 bytes): how its DEM's texels decode to
/// heights, as [`mln_util::dem::DemUnpack`] does, the exaggeration, and how
/// far the surface's skirts reach down.
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
    /// In meters.
    /* 24 */
    pub skirt_length: f32,
    /* 28 */ pub pad0: f32,
    /* 32 */
}
const _: () = assert!(size_of::<TerrainPropsUBO>() == 2 * 16);
