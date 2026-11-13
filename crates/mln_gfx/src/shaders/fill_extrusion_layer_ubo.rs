use bytemuck::{Pod, Zeroable};

/// The fill extrusion shader's per-drawable uniform block (112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillExtrusionDrawableUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub pixel_coord_upper: [f32; 2],
    /*  72 */ pub pixel_coord_lower: [f32; 2],
    /*  80 */ pub height_factor: f32,
    /*  84 */ pub tile_ratio: f32,

    // Interpolations
    /*  88 */ pub base_t: f32,
    /*  92 */ pub height_t: f32,
    /*  96 */ pub color_t: f32,
    /* 100 */ pub pattern_from_t: f32,
    /* 104 */ pub pattern_to_t: f32,
    /* 108 */ pub pad1: f32,
    /* 112 */
}
const _: () = assert!(std::mem::size_of::<FillExtrusionDrawableUBO>() == 7 * 16);

/// The fill extrusion shader's per-tile uniform block (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillExtrusionTilePropsUBO {
    /*  0 */ pub pattern_from: [f32; 4],
    /* 16 */ pub pattern_to: [f32; 4],
    /* 32 */ pub texsize: [f32; 2],
    /* 40 */ pub pad1: f32,
    /* 44 */ pub pad2: f32,
    /* 48 */
}
const _: () = assert!(std::mem::size_of::<FillExtrusionTilePropsUBO>() == 3 * 16);

/// Evaluated properties that do not depend on the tile (80 bytes). `color`
/// is the premultiplied `mln::Color` as r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct FillExtrusionPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub light_color: [f32; 3],
    /* 28 */ pub pad1: f32,
    /* 32 */ pub light_position: [f32; 3],
    /* 44 */ pub base: f32,
    /* 48 */ pub height: f32,
    /* 52 */ pub light_intensity: f32,
    /* 56 */ pub vertical_gradient: f32,
    /* 60 */ pub opacity: f32,
    /* 64 */ pub fade: f32,
    /* 68 */ pub from_scale: f32,
    /* 72 */ pub to_scale: f32,
    /* 76 */ pub pad2: f32,
    /* 80 */
}
const _: () = assert!(std::mem::size_of::<FillExtrusionPropsUBO>() == 5 * 16);
