//! What a program reads the ground's height with, by the height rule of
//! [`mln_util::dem`]: the terrain's uniform block and its DEM texture. Each
//! shading language's module in [`source`](super::source) has the rule as a
//! `ground_height!` snippet, which a vertex stage's source names.

#![expect(unsafe_code, reason = "`uniform_block!` implements `Pod` for the blocks it declares")]

use crate::shaders::program::ProgramTexture;

crate::uniform_block! {
    /// The terrain's uniform block: how its DEM's texels decode to heights,
    /// as [`mln_util::dem::DemUnpack`] does, and the exaggeration.
    pub struct TerrainPropsUBO {
        /// The weights of a texel's red, green and blue channels, from 0 to
        /// 255, then the base added to their sum.
        pub unpack: [f32; 4],
        /// What the sum is multiplied by, to meters.
        pub unpack_scale: f32,
        /// What the heights are multiplied by.
        pub exaggeration: f32,
        #[padding]
        pub pad0: f32,
        #[padding]
        pub pad1: f32,
    }
}

/// The DEM tile that the `ground_height!` snippets read: its RGBA8 texels
/// behind a one-texel border, which the vertex stage reads texel by texel.
/// A program that reads the ground lists it among its textures.
pub const GROUND_DEM: ProgramTexture = ProgramTexture::unfilterable("ground_dem").precise().vertex();
