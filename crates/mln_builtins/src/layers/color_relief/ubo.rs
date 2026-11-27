//! The color reliefs' blocks.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The color relief shader's per-drawable uniform block.
    pub struct ColorReliefDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// The color relief shader's per-tile uniform block.
    pub struct ColorReliefTilePropsUBO {
        /// How the DEM's texels decode, as [`mln_engine::tile::dem::DemUnpack`] does:
        /// the weights of a texel's red, green and blue channels, from 0 to 255,
        /// then the base added to their sum.
        pub unpack: [f32; 4],
        pub dimension: [f32; 2], // Texture dimensions
        pub color_ramp_size: i32, // Number of color stops
        /// What the sum is multiplied by, to meters.
        pub unpack_scale: f32,
    }
}

uniform_block! {
    /// The color relief shader's evaluated properties uniform block.
    pub struct ColorReliefEvaluatedPropsUBO {
        pub opacity: f32,
        pub pad_eval0: f32,
        pub pad_eval1: f32,
        pub pad_eval2: f32,
    }
}
