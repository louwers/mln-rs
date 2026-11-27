//! The lines' blocks.

use bytemuck::{Pod, Zeroable};
use mln_gfx::shaders::block::uniform_block;

//
// Line

uniform_block! {
    /// The line shader's per-drawable uniform block.
    pub struct LineDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub ratio: f32,

        // Interpolations
        pub color_t: f32,
        pub blur_t: f32,
        pub opacity_t: f32,
        pub gapwidth_t: f32,
        pub offset_t: f32,
        pub width_t: f32,
        pub pad1: f32,
    }
}

//
// Line gradient

uniform_block! {
    /// The line gradient shader's per-drawable uniform block.
    pub struct LineGradientDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub ratio: f32,

        // Interpolations
        pub blur_t: f32,
        pub opacity_t: f32,
        pub gapwidth_t: f32,
        pub offset_t: f32,
        pub width_t: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

//
// Line pattern

uniform_block! {
    /// The line pattern shader's per-drawable uniform block.
    pub struct LinePatternDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub ratio: f32,

        // Interpolations
        pub blur_t: f32,
        pub opacity_t: f32,
        pub gapwidth_t: f32,
        pub offset_t: f32,
        pub width_t: f32,
        pub pattern_from_t: f32,
        pub pattern_to_t: f32,
        pub floorwidth_t: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// The line pattern shader's per-tile uniform block.
    pub struct LinePatternTilePropsUBO {
        pub pattern_from: [f32; 4],
        pub pattern_to: [f32; 4],
        pub scale: [f32; 4],
        pub texsize: [f32; 2],
        pub fade: f32,
        pub pad1: f32,
    }
}

//
// Line SDF

uniform_block! {
    /// The line SDF shader's per-drawable uniform block.
    pub struct LineSDFDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub patternscale_a: [f32; 2],
        pub patternscale_b: [f32; 2],
        pub tex_y_a: f32,
        pub tex_y_b: f32,
        pub ratio: f32,

        // Interpolations
        pub color_t: f32,
        pub blur_t: f32,
        pub opacity_t: f32,
        pub gapwidth_t: f32,
        pub offset_t: f32,
        pub width_t: f32,
        pub floorwidth_t: f32,
        /// The height of the bucket's dash atlas, where the dash array differs
        /// by feature: the shaders then read the patterns from an attribute.
        pub atlas_height: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// The line SDF shader's per-tile uniform block.
    pub struct LineSDFTilePropsUBO {
        pub sdfgamma: f32,
        pub mix: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct LineEvaluatedPropsUBO {
        pub color: [f32; 4],
        pub blur: f32,
        pub opacity: f32,
        pub gapwidth: f32,
        pub offset: f32,
        pub width: f32,
        pub floorwidth: f32,
        pub pad0: f32,
        pub pad1: f32,
    }
}

/// Any line per-drawable uniform block, as large as the largest,
/// `LineSDFDrawableUBO`'s 128 bytes: the element of the buffer that the draws
/// of a layer's programs write.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineDrawableUnionUBO {
    bytes: [u8; 128],
}
const _: () = assert!(size_of::<LineDrawableUnionUBO>() == 8 * 16);

/// Any line per-tile uniform block, as large as the largest,
/// `LinePatternTilePropsUBO`'s 64 bytes: the element of the buffer that the
/// draws of a layer's programs write.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineTilePropsUnionUBO {
    bytes: [u8; 64],
}
const _: () = assert!(size_of::<LineTilePropsUnionUBO>() == 4 * 16);
