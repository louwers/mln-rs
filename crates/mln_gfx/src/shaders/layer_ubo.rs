use bytemuck::{Pod, Zeroable};

/// The global paint parameters uniform block (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct GlobalPaintParamsUBO {
    /*  0 */ pub pattern_atlas_texsize: [f32; 2],
    /*  8 */ pub units_to_pixels: [f32; 2],
    /* 16 */ pub world_size: [f32; 2],
    /* 24 */ pub camera_to_center_distance: f32,
    /* 28 */ pub symbol_fade_change: f32,
    /* 32 */ pub aspect_ratio: f32,
    /* 36 */ pub pixel_ratio: f32,
    /* 40 */ pub map_zoom: f32,
    /* 44 */ pub pad1: f32,
    /* 48 */
}
const _: () = assert!(size_of::<GlobalPaintParamsUBO>() == 3 * 16);

/// The per-drawable projection uniform block (208 bytes); see
/// `ProjectionData`. On the globe, `matrix` takes a point on the unit sphere
/// relative to `anchor` to clip space, so the shaders only handle a tile's
/// points as small offsets from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ProjectionUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub fallback_matrix: [f32; 4 * 4],
    /* 128 */ pub tile_mercator_coords: [f32; 4],
    /* 144 */ pub clipping_plane: [f32; 4],
    /* 160 */ pub projection_transition: f32,
    /* 164 */ pub depth_offset: f32,
    /// Tile-unit translation the sphere path adds; the fallback matrix
    /// carries its own.
    /* 168 */
    pub translate: [f32; 2],
    /// The tile's anchor on the unit sphere, and the clipping plane's value
    /// there in W.
    /* 176 */
    pub anchor: [f32; 4],
    /// The sine and cosine of the anchor's longitude, then of its latitude.
    /* 192 */
    pub anchor_angles: [f32; 4],
    /* 208 */
}
const _: () = assert!(size_of::<ProjectionUBO>() == 13 * 16);
