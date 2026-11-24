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

/// The fog over the terrain's surface (48 bytes), from the style's `sky`. It
/// covers a fragment of the surface by how far along the view it is: from
/// none at `ground_blend` of the depth range to all at its far end, in
/// `fog_color` that turns to `horizon_color` past `horizon_blend`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainFogUBO {
    /// Linear, through a gamma of 2.2, and premultiplied.
    /*  0 */
    pub fog_color: [f32; 4],
    /// Linear and premultiplied, as `fog_color`.
    /* 16 */
    pub horizon_color: [f32; 4],
    /// The distances along the view, in pixels, at which the fog's depth is
    /// 0 and 1, the second beyond the first where there is fog. The depth
    /// between them is a perspective depth, which grows fastest near the
    /// first.
    /* 32 */
    pub depth_range: [f32; 2],
    /// Where in the depth the fog starts, from 0 to 1. At 1 there is no fog.
    /* 40 */
    pub ground_blend: f32,
    /// Where in the depth the fog starts to turn to the horizon's color,
    /// from 0 to 1.
    /* 44 */
    pub horizon_blend: f32,
    /* 48 */
}
const _: () = assert!(size_of::<TerrainFogUBO>() == 3 * 16);

impl TerrainFogUBO {
    /// No fog.
    pub const NONE: TerrainFogUBO = TerrainFogUBO {
        fog_color: [0.0; 4],
        horizon_color: [0.0; 4],
        depth_range: [0.0; 2],
        ground_blend: 1.0,
        horizon_blend: 1.0,
    };
}

/// The uniform block of the terrain's programs (80 bytes), at
/// [`ID_TERRAIN_PROPS_UBO`](crate::shaders::shader_defines::ID_TERRAIN_PROPS_UBO):
/// how the surface stands, which its vertex stage reads, and the fog over
/// it, which its fragment stage reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainSurfaceUBO {
    /*  0 */ pub props: TerrainPropsUBO,
    /* 32 */ pub fog: TerrainFogUBO,
    /* 80 */
}
const _: () = assert!(size_of::<TerrainSurfaceUBO>() == 5 * 16);
