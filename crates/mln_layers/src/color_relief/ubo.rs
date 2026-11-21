mln_gfx::uniform_block! {
    /// The color relief shader's per-drawable uniform block.
    pub struct ColorReliefDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

mln_gfx::uniform_block! {
    /// The color relief shader's per-tile uniform block.
    pub struct ColorReliefTilePropsUBO {
        // DEM unpack vector
        pub unpack: [f32; 4],
        // Texture dimensions
        pub dimension: [f32; 2],
        // Number of color stops
        pub color_ramp_size: i32,
        // Padding for alignment
        #[padding]
        pub pad_tile0: f32,
    }
}

mln_gfx::uniform_block! {
    /// The color relief shader's evaluated properties uniform block.
    pub struct ColorReliefEvaluatedPropsUBO {
        pub opacity: f32,
        #[padding]
        pub pad_eval0: f32,
        #[padding]
        pub pad_eval1: f32,
        #[padding]
        pub pad_eval2: f32,
    }
}
