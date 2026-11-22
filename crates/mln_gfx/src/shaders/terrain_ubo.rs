use bytemuck::{Pod, Zeroable};

use crate::shaders::layer_ubo::ProjectionUBO;

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
    /// What the standing layers that terrain hides scale the ground's depth
    /// by, GL JS's in its NDC depth: `2fn / (f − n)` of the view's near plane
    /// `n` and far plane `f`, or 0 in a frame without the ground's depth.
    /* 28 */
    pub depth_factor: f32,
    /* 32 */
}
const _: () = assert!(size_of::<TerrainPropsUBO>() == 2 * 16);

/// Where a standing draw reads its ground (48 bytes): the DEM tile it
/// binds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct GroundDrawableUBO {
    /// Where the draw's tile is in the DEM tile, as
    /// [`TerrainDrawableUBO::dem_coords`] has it. A width of 0 is flat
    /// ground.
    /* 0 */
    pub dem_coords: [f32; 4],
    /// How the DEM decodes, as the surface's.
    /* 16 */
    pub props: TerrainPropsUBO,
    /* 48 */
}
const _: () = assert!(size_of::<GroundDrawableUBO>() == 3 * 16);

/// The entry of [`ID_PROJECTION_UBO`](crate::shaders::shader_defines::ID_PROJECTION_UBO)
/// of a terrain variant's draw (224 bytes): its projection, and the ground
/// it stands on, which every standing program reads there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct TerrainProjectionUBO {
    /*   0 */ pub projection: ProjectionUBO,
    /* 176 */ pub ground: GroundDrawableUBO,
    /* 224 */
}
const _: () = assert!(size_of::<TerrainProjectionUBO>() == 14 * 16);

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
