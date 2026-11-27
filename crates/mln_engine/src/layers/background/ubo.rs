//! The backgrounds' blocks.

use bytemuck::{Pod, Zeroable};
use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The background shader's per-drawable uniform block.
    pub struct BackgroundDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    /// `color` is the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct BackgroundPropsUBO {
        pub color: [f32; 4],
        pub opacity: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// The background pattern shader's per-drawable uniform block.
    pub struct BackgroundPatternDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub pixel_coord_upper: [f32; 2],
        pub pixel_coord_lower: [f32; 2],
        pub tile_units_to_pixels: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct BackgroundPatternPropsUBO {
        pub pattern_tl_a: [f32; 2],
        pub pattern_br_a: [f32; 2],
        pub pattern_tl_b: [f32; 2],
        pub pattern_br_b: [f32; 2],
        pub pattern_size_a: [f32; 2],
        pub pattern_size_b: [f32; 2],
        pub scale_a: f32,
        pub scale_b: f32,
        pub mix: f32,
        pub opacity: f32,
        /// The size of the layer's pattern atlas, in texels.
        pub texsize: [f32; 2],
        pub pad1: f32,
        pub pad2: f32,
    }
}

/// Either background per-drawable uniform block, as large as the larger,
/// 96 bytes: the element of the buffer that both programs' draws write.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundDrawableUnionUBO {
    bytes: [u8; 96],
}
const _: () = assert!(size_of::<BackgroundDrawableUnionUBO>() == 6 * 16);
