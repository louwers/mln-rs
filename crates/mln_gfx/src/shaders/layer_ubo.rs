use crate::shaders::block::uniform_block;

uniform_block! {
    /// The global paint parameters uniform block.
    pub struct GlobalPaintParamsUBO {
        pub units_to_pixels: [f32; 2],
        pub world_size: [f32; 2],
        pub camera_to_center_distance: f32,
        pub symbol_fade_change: f32,
        pub aspect_ratio: f32,
        pub pixel_ratio: f32,
        pub map_zoom: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// The per-drawable projection uniform block; see
    /// `ProjectionData`. On the globe, `matrix` takes a point on the unit sphere
    /// relative to `anchor` to clip space, so the shaders only handle a tile's
    /// points as small offsets from it.
    pub struct ProjectionUBO {
        pub matrix: [f32; 4 * 4],
        pub fallback_matrix: [f32; 4 * 4],
        pub tile_mercator_coords: [f32; 4],
        pub clipping_plane: [f32; 4],
        pub projection_transition: f32,
        pub depth_offset: f32,
        /// Tile-unit translation the sphere path adds; the fallback matrix
        /// carries its own.
        pub translate: [f32; 2],
        /// The tile's anchor on the unit sphere, and the clipping plane's value
        /// there in W.
        pub anchor: [f32; 4],
        /// The sine and cosine of the anchor's longitude, then of its latitude.
        pub anchor_angles: [f32; 4],
    }
}
