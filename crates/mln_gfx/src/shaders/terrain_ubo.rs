use bytemuck::{Pod, Zeroable};

/// A terrain tile's draw block (80 bytes), an entry of the storage buffer
/// that the terrain's draws index by `globalIndex`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainDrawableUBO {
    /// From the tile's units and meters to clip space.
    /*  0 */
    pub matrix: [f32; 4 * 4],
    /// Where the tile reads its DEM: the scale and offset that map the
    /// tile's uv to the DEM's (`uv * scale + offset`), then the DEM's width
    /// in pixels. A width of 0 is flat ground.
    /* 64 */
    pub dem_coords: [f32; 4],
    /* 80 */
}
const _: () = assert!(size_of::<TerrainDrawableUBO>() == 5 * 16);

/// The terrain's uniform block (32 bytes): how its DEM's texels decode to
/// heights, the exaggeration, and how far the skirts reach down.
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
    /* 28 */ pub pad: f32,
    /* 32 */
}
const _: () = assert!(size_of::<TerrainPropsUBO>() == 2 * 16);

/// The terrain's fog (64 bytes), GL JS's over the surface, which only the
/// surface's fragment stage reads. It tints what a stack's draw covers, by
/// the colors' alpha, and leaves its coverage as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainFogUBO {
    /// Linear, through a gamma of 2.2, and premultiplied.
    /*  0 */
    pub fog_color: [f32; 4],
    /// Linear and premultiplied, as `fog_color`: the sky's `horizon-color`,
    /// which the fog turns to past `horizon_blend`.
    /* 16 */
    pub horizon_color: [f32; 4],
    /// Where the fog's depth runs from 0 to 1: the near and far distances
    /// along the view, in pixels, the far one beyond the near one where
    /// `opacity` isn't 0.
    /* 32 */
    pub depth_range: [f32; 2],
    /// 0 to 1, below 1 with fog.
    /* 40 */
    pub ground_blend: f32,
    /// 0 to 1.
    /* 44 */
    pub horizon_blend: f32,
    /// 0 turns the fog off.
    /* 48 */
    pub opacity: f32,
    /* 52 */ pub pad: [f32; 3],
    /* 64 */
}
const _: () = assert!(size_of::<TerrainFogUBO>() == 4 * 16);
