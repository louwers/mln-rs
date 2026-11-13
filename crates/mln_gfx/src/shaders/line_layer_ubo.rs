use bytemuck::{Pod, Zeroable};

//
// Line

/// The line shader's per-drawable uniform block (96 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub ratio: f32,

    // Interpolations
    /* 68 */ pub color_t: f32,
    /* 72 */ pub blur_t: f32,
    /* 76 */ pub opacity_t: f32,
    /* 80 */ pub gapwidth_t: f32,
    /* 84 */ pub offset_t: f32,
    /* 88 */ pub width_t: f32,
    /* 92 */ pub pad1: f32,
    /* 96 */
}
const _: () = assert!(size_of::<LineDrawableUBO>() == 6 * 16);

//
// Line gradient

/// The line gradient shader's per-drawable uniform block (96 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineGradientDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub ratio: f32,

    // Interpolations
    /* 68 */ pub blur_t: f32,
    /* 72 */ pub opacity_t: f32,
    /* 76 */ pub gapwidth_t: f32,
    /* 80 */ pub offset_t: f32,
    /* 84 */ pub width_t: f32,
    /* 88 */ pub pad1: f32,
    /* 92 */ pub pad2: f32,
    /* 96 */
}
const _: () = assert!(size_of::<LineGradientDrawableUBO>() == 6 * 16);

//
// Line pattern

/// The line pattern shader's per-drawable uniform block (112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LinePatternDrawableUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub ratio: f32,

    // Interpolations
    /*  68 */ pub blur_t: f32,
    /*  72 */ pub opacity_t: f32,
    /*  76 */ pub gapwidth_t: f32,
    /*  80 */ pub offset_t: f32,
    /*  84 */ pub width_t: f32,
    /*  88 */ pub pattern_from_t: f32,
    /*  92 */ pub pattern_to_t: f32,
    /*  96 */ pub floorwidth_t: f32,
    /* 100 */ pub pad1: f32,
    /* 104 */ pub pad2: f32,
    /* 108 */ pub pad3: f32,
    /* 112 */
}
const _: () = assert!(size_of::<LinePatternDrawableUBO>() == 7 * 16);

/// The line pattern shader's per-tile uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LinePatternTilePropsUBO {
    /*  0 */ pub pattern_from: [f32; 4],
    /* 16 */ pub pattern_to: [f32; 4],
    /* 32 */ pub scale: [f32; 4],
    /* 48 */ pub texsize: [f32; 2],
    /* 56 */ pub fade: f32,
    /* 60 */ pub pad1: f32,
    /* 64 */
}
const _: () = assert!(size_of::<LinePatternTilePropsUBO>() == 4 * 16);

//
// Line SDF

/// The line SDF shader's per-drawable uniform block (128 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineSDFDrawableUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub patternscale_a: [f32; 2],
    /*  72 */ pub patternscale_b: [f32; 2],
    /*  80 */ pub tex_y_a: f32,
    /*  84 */ pub tex_y_b: f32,
    /*  88 */ pub ratio: f32,

    // Interpolations
    /*  92 */ pub color_t: f32,
    /*  96 */ pub blur_t: f32,
    /* 100 */ pub opacity_t: f32,
    /* 104 */ pub gapwidth_t: f32,
    /* 108 */ pub offset_t: f32,
    /* 112 */ pub width_t: f32,
    /* 116 */ pub floorwidth_t: f32,
    /* 120 */ pub pad1: f32,
    /* 124 */ pub pad2: f32,
    /* 128 */
}
const _: () = assert!(size_of::<LineSDFDrawableUBO>() == 8 * 16);

/// The line SDF shader's per-tile uniform block (16 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineSDFTilePropsUBO {
    /*  0 */ pub sdfgamma: f32,
    /*  4 */ pub mix: f32,
    /*  8 */ pub pad1: f32,
    /* 12 */ pub pad2: f32,
    /* 16 */
}
const _: () = assert!(size_of::<LineSDFTilePropsUBO>() == 16);

/// Bit flags for the line properties that are expressions.
pub mod line_expression_mask {
    pub const NONE: u32 = 0;
    pub const COLOR: u32 = 1 << 0;
    pub const OPACITY: u32 = 1 << 1;
    pub const BLUR: u32 = 1 << 2;
    pub const WIDTH: u32 = 1 << 3;
    pub const GAP_WIDTH: u32 = 1 << 4;
    pub const FLOOR_WIDTH: u32 = 1 << 5;
    pub const OFFSET: u32 = 1 << 6;
}

/// The size of `LineExpressionUBO`: seven `gfx::GPUExpression`s of 224
/// bytes each. The Metal line shaders read it for the properties set in
/// `LineEvaluatedPropsUBO::expression_mask`; with no GPU expressions, the
/// renderer binds zeros.
pub const LINE_EXPRESSION_UBO_SIZE: usize = 7 * (32 + (4 + 8) * 16);

/// Evaluated properties that do not depend on the tile (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineEvaluatedPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub blur: f32,
    /* 20 */ pub opacity: f32,
    /* 24 */ pub gapwidth: f32,
    /* 28 */ pub offset: f32,
    /* 32 */ pub width: f32,
    /* 36 */ pub floorwidth: f32,
    /* 40 */ pub expression_mask: u32,
    /* 44 */ pub pad1: f32,
    /* 48 */
}
const _: () = assert!(size_of::<LineEvaluatedPropsUBO>() == 3 * 16);

/// Any line per-drawable uniform block, for consolidated UBOs: 128 bytes,
/// the size of `LineSDFDrawableUBO`.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineDrawableUnionUBO {
    bytes: [u8; 128],
}
const _: () = assert!(size_of::<LineDrawableUnionUBO>() == 8 * 16);

impl Default for LineDrawableUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl LineDrawableUnionUBO {
    /// Assigns one union member (`drawableUBOVector[i].lineDrawableUBO = ...`).
    pub fn set<T: Pod>(&mut self, member: &T) {
        let src = bytemuck::bytes_of(member);
        self.bytes[..src.len()].copy_from_slice(src);
    }
}

/// Any line per-tile uniform block, for consolidated UBOs: 64 bytes, the
/// size of `LinePatternTilePropsUBO`.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineTilePropsUnionUBO {
    bytes: [u8; 64],
}
const _: () = assert!(size_of::<LineTilePropsUnionUBO>() == 4 * 16);

impl Default for LineTilePropsUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl LineTilePropsUnionUBO {
    /// Assigns one union member.
    pub fn set<T: Pod>(&mut self, member: &T) {
        let src = bytemuck::bytes_of(member);
        self.bytes[..src.len()].copy_from_slice(src);
    }
}
