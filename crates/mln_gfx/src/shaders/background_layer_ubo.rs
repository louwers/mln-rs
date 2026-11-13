use bytemuck::{Pod, Zeroable};

/// The background shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(size_of::<BackgroundDrawableUBO>() == 4 * 16);

/// Evaluated properties that do not depend on the tile (32 bytes).
/// `color` is the premultiplied `mln::Color` as r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub opacity: f32,
    /* 20 */ pub pad1: f32,
    /* 24 */ pub pad2: f32,
    /* 28 */ pub pad3: f32,
    /* 32 */
}
const _: () = assert!(size_of::<BackgroundPropsUBO>() == 2 * 16);

/// The background pattern shader's per-drawable uniform block (96 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundPatternDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub pixel_coord_upper: [f32; 2],
    /* 72 */ pub pixel_coord_lower: [f32; 2],
    /* 80 */ pub tile_units_to_pixels: f32,
    /* 84 */ pub pad1: f32,
    /* 88 */ pub pad2: f32,
    /* 92 */ pub pad3: f32,
    /* 96 */
}
const _: () = assert!(size_of::<BackgroundPatternDrawableUBO>() == 6 * 16);

/// Evaluated properties that do not depend on the tile (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundPatternPropsUBO {
    /*  0 */ pub pattern_tl_a: [f32; 2],
    /*  8 */ pub pattern_br_a: [f32; 2],
    /* 16 */ pub pattern_tl_b: [f32; 2],
    /* 24 */ pub pattern_br_b: [f32; 2],
    /* 32 */ pub pattern_size_a: [f32; 2],
    /* 40 */ pub pattern_size_b: [f32; 2],
    /* 48 */ pub scale_a: f32,
    /* 52 */ pub scale_b: f32,
    /* 56 */ pub mix: f32,
    /* 60 */ pub opacity: f32,
    /* 64 */
}
const _: () = assert!(size_of::<BackgroundPatternPropsUBO>() == 4 * 16);

/// Either background per-drawable uniform block, for consolidated UBOs: a C++
/// union, the size of the larger member, 96 bytes. Write either member with
/// `from_drawable` / `from_pattern_drawable`; the rest stays zero (C++
/// value-initializes the vector elements).
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct BackgroundDrawableUnionUBO {
    bytes: [u8; 96],
}
const _: () = assert!(size_of::<BackgroundDrawableUnionUBO>() == 6 * 16);

impl Default for BackgroundDrawableUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl BackgroundDrawableUnionUBO {
    #[must_use]
    pub fn from_drawable(ubo: &BackgroundDrawableUBO) -> Self {
        let mut u = Self::zeroed();
        u.bytes[..64].copy_from_slice(bytemuck::bytes_of(ubo));
        u
    }

    #[must_use]
    pub fn from_pattern_drawable(ubo: &BackgroundPatternDrawableUBO) -> Self {
        let mut u = Self::zeroed();
        u.bytes.copy_from_slice(bytemuck::bytes_of(ubo));
        u
    }
}
