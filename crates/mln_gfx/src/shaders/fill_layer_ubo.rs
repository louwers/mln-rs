use bytemuck::{Pod, Zeroable};

//
// Fill

/// The fill shader's per-drawable uniform block (80 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],

    // Interpolations
    /* 64 */ pub color_t: f32,
    /* 68 */ pub opacity_t: f32,
    /* 72 */ pub pad1: f32,
    /* 76 */ pub pad2: f32,
    /* 80 */
}
const _: () = assert!(std::mem::size_of::<FillDrawableUBO>() == 5 * 16);

//
// Fill outline

/// The fill outline shader's per-drawable uniform block (80 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillOutlineDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],

    // Interpolations
    /* 64 */ pub outline_color_t: f32,
    /* 68 */ pub opacity_t: f32,
    /* 72 */ pub pad1: f32,
    /* 76 */ pub pad2: f32,
    /* 80 */
}
const _: () = assert!(std::mem::size_of::<FillOutlineDrawableUBO>() == 5 * 16);

//
// Fill pattern

/// The fill pattern shader's per-drawable uniform block (112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillPatternDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub pixel_coord_upper: [f32; 2],
    /* 72 */ pub pixel_coord_lower: [f32; 2],
    /* 80 */ pub tile_ratio: f32,

    // Interpolations
    /* 84 */ pub pattern_from_t: f32,
    /* 88 */ pub pattern_to_t: f32,
    /* 92 */ pub opacity_t: f32,
    /* 96 */ pub color_t: f32,
    /*100 */ pub pad1: f32,
    /*104 */ pub pad2: f32,
    /*108 */ pub pad3: f32,
    /*112 */
}
const _: () = assert!(std::mem::size_of::<FillPatternDrawableUBO>() == 7 * 16);

/// The fill pattern shader's per-tile uniform block (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillPatternTilePropsUBO {
    /*  0 */ pub pattern_from: [f32; 4],
    /* 16 */ pub pattern_to: [f32; 4],
    /* 32 */ pub texsize: [f32; 2],
    /* 40 */ pub sdf: f32,
    /* 44 */ pub pad2: f32,
    /* 48 */
}
const _: () = assert!(std::mem::size_of::<FillPatternTilePropsUBO>() == 3 * 16);

//
// Fill pattern outline

/// The fill outline pattern shader's per-drawable uniform block, laid out as
/// `FillPatternDrawableUBO`.
pub type FillOutlinePatternDrawableUBO = FillPatternDrawableUBO;

/// The fill outline pattern shader's per-tile uniform block, laid out as
/// `FillPatternTilePropsUBO`.
pub type FillOutlinePatternTilePropsUBO = FillPatternTilePropsUBO;

//
// Fill outline triangulated

/// The fill outline triangulated shader's per-drawable uniform block (80
/// bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillOutlineTriangulatedDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub ratio: f32,
    /* 68 */ pub pad1: f32,
    /* 72 */ pub pad2: f32,
    /* 76 */ pub pad3: f32,
    /* 80 */
}
const _: () = assert!(std::mem::size_of::<FillOutlineTriangulatedDrawableUBO>() == 5 * 16);

/// Evaluated properties that do not depend on the tile (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillEvaluatedPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub outline_color: [f32; 4],
    /* 32 */ pub opacity: f32,
    /* 36 */ pub fade: f32,
    /* 40 */ pub from_scale: f32,
    /* 44 */ pub to_scale: f32,
    /* 48 */
}
const _: () = assert!(std::mem::size_of::<FillEvaluatedPropsUBO>() == 3 * 16);

/// Any fill per-drawable uniform block, for consolidated UBOs: a C++ union, the
/// size of the largest member, 112 bytes. Members are written with the `from_*`
/// constructors; the rest stays zero (C++ value-initializes the vector).
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillDrawableUnionUBO {
    bytes: [u8; 112],
}
const _: () = assert!(std::mem::size_of::<FillDrawableUnionUBO>() == 7 * 16);

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
    pub fn set_fill_outline_triangulated(&mut self, ubo: &FillOutlineTriangulatedDrawableUBO) {
        self.write(ubo);
    }
}

/// Any fill per-tile uniform block, for consolidated UBOs (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillTilePropsUnionUBO {
    pub fill_pattern_tile_props_ubo: FillPatternTilePropsUBO,
}
const _: () = assert!(std::mem::size_of::<FillTilePropsUnionUBO>() == 3 * 16);
