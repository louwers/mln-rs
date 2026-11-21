mln_gfx::uniform_block! {
    /// The raster shader's per-drawable uniform block.
    pub struct RasterDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile.
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
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}
