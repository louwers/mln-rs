/// Maximum number of illumination sources supported
pub const MAX_ILLUMINATION_SOURCES: usize = 4;

mln_gfx::uniform_block! {
    /// The hillshade shader's per-drawable uniform block.
    pub struct HillshadeDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

mln_gfx::uniform_block! {
    /// The hillshade shader's per-tile uniform block.
    pub struct HillshadeTilePropsUBO {
        pub latrange: [f32; 2],
        pub exaggeration: f32,
        pub method: i32,
        pub num_lights: i32,
        #[padding]
        pub pad0: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile. The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct HillshadeEvaluatedPropsUBO {
        pub accent: [f32; 4],
        pub altitudes: [f32; 4],
        pub azimuths: [f32; 4],
        pub shadows: [[f32; 4]; 4],
        pub highlights: [[f32; 4]; 4],
    }
}

mln_gfx::uniform_block! {
    /// The hillshade prepare shader's per-drawable uniform block.
    pub struct HillshadePrepareDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

mln_gfx::uniform_block! {
    /// The hillshade prepare shader's per-tile uniform block.
    pub struct HillshadePrepareTilePropsUBO {
        /// How the DEM's texels decode, as [`mln_util::dem::DemUnpack`]
        /// does: the weights of a texel's red, green and blue channels,
        /// from 0 to 255, then the base added to their sum.
        pub unpack: [f32; 4],
        pub dimension: [f32; 2],
        pub zoom: f32,
        pub maxzoom: f32,
        /// What the sum is multiplied by, to meters.
        pub unpack_scale: f32,
        #[padding]
        pub pad0: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}
