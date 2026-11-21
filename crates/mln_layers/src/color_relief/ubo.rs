mln_gfx::uniform_block! {
    /// The color relief shader's per-drawable uniform block (64 bytes).
    pub struct ColorReliefDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}
const _: () = assert!(size_of::<ColorReliefDrawableUBO>() == 64);

mln_gfx::uniform_block! {
    /// The color relief shader's per-tile uniform block (32 bytes).
    pub struct ColorReliefTilePropsUBO {
        // DEM unpack vector
        pub unpack: [f32; 4],
        // Texture dimensions
        pub dimension: [f32; 2],
        // Number of color stops
        pub color_ramp_size: i32,
        // Padding for alignment
        pub pad_tile0: f32,
    }
}
const _: () = assert!(size_of::<ColorReliefTilePropsUBO>() == 32);

mln_gfx::uniform_block! {
    /// The color relief shader's evaluated properties uniform block (16 bytes).
    pub struct ColorReliefEvaluatedPropsUBO {
        pub opacity: f32,
        pub pad_eval0: f32,
        pub pad_eval1: f32,
        pub pad_eval2: f32,
    }
}
const _: () = assert!(size_of::<ColorReliefEvaluatedPropsUBO>() == 16);
