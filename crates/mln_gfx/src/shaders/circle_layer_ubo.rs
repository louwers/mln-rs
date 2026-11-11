// Ports include/mln/shaders/circle_layer_ubo.hpp

use bytemuck::{Pod, Zeroable};

/// `struct alignas(16) CircleDrawableUBO` (112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct CircleDrawableUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub extrude_scale: [f32; 2],

    // Interpolations
    /*  72 */ pub color_t: f32,
    /*  76 */ pub radius_t: f32,
    /*  80 */ pub blur_t: f32,
    /*  84 */ pub opacity_t: f32,
    /*  88 */ pub stroke_color_t: f32,
    /*  92 */ pub stroke_width_t: f32,
    /*  96 */ pub stroke_opacity_t: f32,
    /* 100 */ pub globe_extrude_scale: f32,
    /* 104 */ pub pad2: f32,
    /* 108 */ pub pad3: f32,
    /* 112 */
}
const _: () = assert!(std::mem::size_of::<CircleDrawableUBO>() == 7 * 16);

/// Evaluated properties that do not depend on the tile (64 bytes). The
/// colors are the premultiplied `mln::Color` as r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct CircleEvaluatedPropsUBO {
    /*  0 */ pub color: [f32; 4],
    /* 16 */ pub stroke_color: [f32; 4],
    /* 32 */ pub radius: f32,
    /* 36 */ pub blur: f32,
    /* 40 */ pub opacity: f32,
    /* 44 */ pub stroke_width: f32,
    /* 48 */ pub stroke_opacity: f32,
    /* 52 */ pub scale_with_map: i32,
    /* 56 */ pub pitch_with_map: i32,
    /* 60 */ pub pad1: f32,
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<CircleEvaluatedPropsUBO>() == 4 * 16);
