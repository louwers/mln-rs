//! The engine's uniform blocks, which any program may read.

#![expect(unsafe_code, reason = "`uniform_block!` implements `Pod` for the blocks it declares")]

crate::uniform_block! {
    /// The view's uniform block (32 bytes), what every draw of a frame may read
    /// of the view it draws: the view's pixels per clip space unit, the size of
    /// the render target in device pixels, the camera's distance to the view's
    /// center in pixels, the view's aspect ratio and the target's device pixels
    /// per logical pixel.
    pub struct ViewUBO {
        pub units_to_pixels: [f32; 2],
        pub target_size: [f32; 2],
        pub camera_to_center_distance: f32,
        pub aspect_ratio: f32,
        pub pixel_ratio: f32,
        pub pad1: f32,
    }
}
const _: () = assert!(size_of::<ViewUBO>() == 2 * 16);

crate::uniform_block! {
    /// The per-drawable projection uniform block (176 bytes); see
    /// `ProjectionData`.
    #[glsl_es(prefix = "u_projection_")]
    pub struct ProjectionUBO {
        pub matrix: [f32; 4 * 4],
        pub fallback_matrix: [f32; 4 * 4],
        pub tile_mercator_coords: [f32; 4],
        pub clipping_plane: [f32; 4],
        pub transition: f32,
        pub depth_offset: f32,
        /// Tile-unit translation the sphere path adds; the fallback matrix
        /// carries its own.
        pub translate: [f32; 2],
    }
}
const _: () = assert!(size_of::<ProjectionUBO>() == 11 * 16);
