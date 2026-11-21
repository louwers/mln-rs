use bytemuck::{Pod, Zeroable};

/// The view's uniform block (32 bytes), what every draw of a frame may read
/// of the view it draws: the view's pixels per clip space unit, the size of
/// the render target in device pixels, the camera's distance to the view's
/// center in pixels, the view's aspect ratio and the target's device pixels
/// per logical pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ViewUBO {
    /*  0 */ pub units_to_pixels: [f32; 2],
    /*  8 */ pub target_size: [f32; 2],
    /* 16 */ pub camera_to_center_distance: f32,
    /* 20 */ pub aspect_ratio: f32,
    /* 24 */ pub pixel_ratio: f32,
    /* 28 */ pub pad1: f32,
    /* 32 */
}
const _: () = assert!(size_of::<ViewUBO>() == 2 * 16);

/// The per-drawable projection uniform block (176 bytes); see
/// `ProjectionData`.
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
    /* 176 */
}
const _: () = assert!(size_of::<ProjectionUBO>() == 11 * 16);
