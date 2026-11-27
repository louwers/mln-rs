//! The rasters' blocks.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The raster shader's per-drawable uniform block.
    pub struct RasterDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile, and the quad the
    /// vertices' unit square (0 to 8192 on both axes) draws on.
    pub struct RasterEvaluatedPropsUBO {
        pub spin_weights: [f32; 4],
        pub tl_parent: [f32; 2],
        pub scale_parent: f32,
        pub buffer_scale: f32,
        pub fade_t: f32,
        pub opacity: f32,
        pub brightness_low: f32,
        pub brightness_high: f32,
        pub saturation_factor: f32,
        pub contrast_factor: f32,
        pub pad1: f32,
        pub pad2: f32,
        /// The quad's top left corner (x, y) and top right corner (z, w), in
        /// tile units.
        pub coords_top: [f32; 4],
        /// Its bottom left corner (x, y) and bottom right corner (z, w).
        pub coords_bottom: [f32; 4],
        /// The perspective terms of the projective warp from the unit square
        /// onto the quad (x, y), and how far it is blended towards the bilinear
        /// warp (z): 0, 0, 1 for a parallelogram such as a tile's own square.
        pub image_warp: [f32; 4],
    }
}
