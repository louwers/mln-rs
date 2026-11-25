//! The hillshades' blocks, and the hillshade preparation's. The lights
//! that a hillshade shades its slopes with are the terrain's
//! ([`crate::shaders::terrain::ubo`]), whose surface shades a drape of a
//! hillshade's slopes with them too.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The hillshade shader's per-drawable uniform block.
    pub struct HillshadeDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// The hillshade prepare shader's per-drawable uniform block.
    pub struct HillshadePrepareDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// The hillshade prepare shader's per-tile uniform block.
    pub struct HillshadePrepareTilePropsUBO {
        /// How the DEM's texels decode, as [`mln_util::dem::DemUnpack`] does:
        /// the weights of a texel's red, green and blue channels, from 0 to 255,
        /// then the base added to their sum.
        pub unpack: [f32; 4],
        pub dimension: [f32; 2],
        pub zoom: f32,
        pub maxzoom: f32,
        /// What the sum is multiplied by, to meters.
        pub unpack_scale: f32,
        pub pad0: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}
