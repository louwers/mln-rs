use bytemuck::{Pod, Zeroable};

/// The symbol shader's per-drawable uniform block (272 bytes). The C++ tail
/// padding is spelled out as `pad1..3`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct SymbolDrawableUBO {
    /*   0 */ pub matrix: [f32; 4 * 4],
    /*  64 */ pub label_plane_matrix: [f32; 4 * 4],
    /* 128 */ pub coord_matrix: [f32; 4 * 4],
    /* 192 */ pub texsize: [f32; 2],
    /* 200 */ pub texsize_icon: [f32; 2],
    /* 208 */ pub is_text_prop: i32,
    /* 212 */ pub rotate_symbol: i32,
    /* 216 */ pub pitch_with_map: i32,
    /* 220 */ pub is_size_zoom_constant: i32,
    /* 224 */ pub is_size_feature_constant: i32,
    /* 228 */ pub is_offset: i32,
    /* 232 */ pub size_t: f32,
    /* 236 */ pub size: f32,
    // Interpolations
    /* 240 */ pub fill_color_t: f32,
    /* 244 */ pub halo_color_t: f32,
    /* 248 */ pub opacity_t: f32,
    /* 252 */ pub halo_width_t: f32,
    /* 256 */ pub halo_blur_t: f32,
    /* 260 */ pub is_along_line: i32,
    /* 264 */ pub is_variable_anchor: i32,
    /* 268 */ pub pitched_scale: f32,
    /* 272 */ pub translation: [f32; 2],
    /* 280 */ pub pad1: f32,
    /* 284 */ pub pad2: f32,
    /* 288 */
}
const _: () = assert!(std::mem::size_of::<SymbolDrawableUBO>() == 18 * 16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct SymbolTilePropsUBO {
    /*  0 */ pub is_text: i32,
    /*  4 */ pub is_halo: i32,
    /*  8 */ pub gamma_scale: f32,
    /* 12 */ pub pad1: f32,
    /* 16 */
}
const _: () = assert!(std::mem::size_of::<SymbolTilePropsUBO>() == 16);

/// The symbol shader's evaluated properties uniform block. The colors are
/// premultiplied r, g, b, a floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct SymbolEvaluatedPropsUBO {
    /*  0 */ pub text_fill_color: [f32; 4],
    /* 16 */ pub text_halo_color: [f32; 4],
    /* 32 */ pub text_opacity: f32,
    /* 36 */ pub text_halo_width: f32,
    /* 40 */ pub text_halo_blur: f32,
    /* 44 */ pub pad1: f32,
    /* 48 */ pub icon_fill_color: [f32; 4],
    /* 64 */ pub icon_halo_color: [f32; 4],
    /* 80 */ pub icon_opacity: f32,
    /* 84 */ pub icon_halo_width: f32,
    /* 88 */ pub icon_halo_blur: f32,
    /* 92 */ pub pad2: f32,
    /* 96 */
}
const _: () = assert!(std::mem::size_of::<SymbolEvaluatedPropsUBO>() == 6 * 16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct CollisionDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */
}
const _: () = assert!(std::mem::size_of::<CollisionDrawableUBO>() == 4 * 16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct CollisionTilePropsUBO {
    /*  0 */ pub extrude_scale: [f32; 2],
    /*  8 */ pub overscale_factor: f32,
    /* 12 */ pub pad1: f32,
    /* 16 */
}
const _: () = assert!(std::mem::size_of::<CollisionTilePropsUBO>() == 16);
