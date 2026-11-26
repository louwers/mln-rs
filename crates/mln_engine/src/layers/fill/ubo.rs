//! The fills' blocks.

use bytemuck::{Pod, Zeroable};
use mln_gfx::shaders::block::uniform_block;

//
// Fill

uniform_block! {
    /// The fill shader's per-draw uniform block.
    pub struct FillDrawableUBO {
        pub matrix: [f32; 4 * 4],

        // Interpolations
        pub color_t: f32,
        pub opacity_t: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

//
// Fill outline

uniform_block! {
    /// The fill outline shader's per-draw uniform block.
    pub struct FillOutlineDrawableUBO {
        pub matrix: [f32; 4 * 4],

        // Interpolations
        pub outline_color_t: f32,
        pub opacity_t: f32,
        /// Where `FillOutlinePulledShader` finds a vertex's paint values in the
        /// buffer it reads them from, in words, a byte each from the lowest:
        /// how many a vertex takes, then the offset in them of its outline
        /// color and its opacity.
        pub paint_layout: u32,
        pub pad2: f32,
    }
}

//
// Fill pattern

uniform_block! {
    /// The fill pattern shader's per-draw uniform block.
    pub struct FillPatternDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub pixel_coord_upper: [f32; 2],
        pub pixel_coord_lower: [f32; 2],
        pub tile_ratio: f32,

        // Interpolations
        pub pattern_from_t: f32,
        pub pattern_to_t: f32,
        pub opacity_t: f32,
        pub color_t: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// The fill pattern shader's per-tile uniform block.
    pub struct FillPatternTilePropsUBO {
        pub pattern_from: [f32; 4],
        pub pattern_to: [f32; 4],
        pub texsize: [f32; 2],
        pub sdf: f32,
        pub pad2: f32,
    }
}

//
// Fill pattern outline

/// The fill outline pattern shader's per-draw uniform block, laid out as
/// `FillPatternDrawableUBO`.
pub type FillOutlinePatternDrawableUBO = FillPatternDrawableUBO;

/// The fill outline pattern shader's per-tile uniform block, laid out as
/// `FillPatternTilePropsUBO`.
pub type FillOutlinePatternTilePropsUBO = FillPatternTilePropsUBO;

uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct FillEvaluatedPropsUBO {
        pub color: [f32; 4],
        pub outline_color: [f32; 4],
        pub opacity: f32,
        pub fade: f32,
        pub from_scale: f32,
        pub to_scale: f32,
    }
}

/// Any fill per-draw uniform block, for consolidated UBOs: the bytes of one of
/// them, the size of the largest, 112 bytes. Members are written with the
/// `from_*` constructors; the rest stays zero.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillDrawableUnionUBO {
    bytes: [u8; 112],
}
const _: () = assert!(size_of::<FillDrawableUnionUBO>() == 7 * 16);

impl Default for FillDrawableUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl FillDrawableUnionUBO {
    fn write<T: Pod>(&mut self, member: &T) {
        let src = bytemuck::bytes_of(member);
        self.bytes[..src.len()].copy_from_slice(src);
    }
    pub fn set_fill(&mut self, ubo: &FillDrawableUBO) {
        self.write(ubo);
    }
    pub fn set_fill_outline(&mut self, ubo: &FillOutlineDrawableUBO) {
        self.write(ubo);
    }
    pub fn set_fill_pattern(&mut self, ubo: &FillPatternDrawableUBO) {
        self.write(ubo);
    }
}

/// Any fill per-tile uniform block, for consolidated UBOs (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillTilePropsUnionUBO {
    pub fill_pattern_tile_props_ubo: FillPatternTilePropsUBO,
}
const _: () = assert!(size_of::<FillTilePropsUnionUBO>() == 3 * 16);
