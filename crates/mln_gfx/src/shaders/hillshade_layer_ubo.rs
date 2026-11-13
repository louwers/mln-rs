use bytemuck::{Pod, Zeroable};

/// Maximum number of illumination sources supported
pub const MAX_ILLUMINATION_SOURCES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum HillshadeMethod {
    Standard = 0,
    Combined = 1,
    Igor = 2,
    Multidirectional = 3,
    Basic = 4,
}

/// The hillshade shader's per-drawable uniform block (64 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadeDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<HillshadeDrawableUBO>() == 4 * 16);

/// The hillshade shader's per-tile uniform block (32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadeTilePropsUBO {
    /*  0 */ pub latrange: [f32; 2],
    /*  8 */ pub exaggeration: f32,
    /* 12 */ pub method: i32,
    /* 16 */ pub num_lights: i32,
    /* 20 */ pub pad0: f32,
    /* 24 */ pub pad1: f32,
    /* 28 */ pub pad2: f32,
    /* 32 */
}
const _: () = assert!(std::mem::size_of::<HillshadeTilePropsUBO>() == 32);

/// Evaluated properties that do not depend on the tile (176 bytes). The
/// colors are the premultiplied `mln::Color` as r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HillshadeEvaluatedPropsUBO {
    /*   0 */ pub accent: [f32; 4],
    /*  16 */ pub altitudes: [f32; 4],
    /*  32 */ pub azimuths: [f32; 4],
    /*  48 */ pub shadows: [f32; 16],
    /* 112 */ pub highlights: [f32; 16],
    /* 176 */
}
const _: () = assert!(std::mem::size_of::<HillshadeEvaluatedPropsUBO>() == 176);
