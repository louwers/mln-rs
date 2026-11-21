use mln_gfx::shaders::block::Bool32;

mln_gfx::uniform_block! {
    /// The symbol shader's per-drawable uniform block (288 bytes).
    pub struct SymbolDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub label_plane_matrix: [f32; 4 * 4],
        pub coord_matrix: [f32; 4 * 4],
        pub texsize: [f32; 2],
        pub texsize_icon: [f32; 2],
        pub is_text_prop: Bool32,
        pub rotate_symbol: Bool32,
        pub pitch_with_map: Bool32,
        pub is_size_zoom_constant: Bool32,
        pub is_size_feature_constant: Bool32,
        pub is_offset: Bool32,
        pub size_t: f32,
        pub size: f32,
        // Interpolations
        pub fill_color_t: f32,
        pub halo_color_t: f32,
        pub opacity_t: f32,
        pub halo_width_t: f32,
        pub halo_blur_t: f32,
        pub is_along_line: Bool32,
        pub is_variable_anchor: Bool32,
        pub pitched_scale: f32,
        pub translation: [f32; 2],
        pub gamma_scale: f32,
        pub is_halo_pass: Bool32,
    }
}
const _: () = assert!(size_of::<SymbolDrawableUBO>() == 18 * 16);

mln_gfx::uniform_block! {
    pub struct SymbolTilePropsUBO {
        pub is_text: Bool32,
        pub is_halo: Bool32,
        pub pad1: f32,
        pub pad2: f32,
    }
}
const _: () = assert!(size_of::<SymbolTilePropsUBO>() == 16);

mln_gfx::uniform_block! {
    /// The symbol shader's evaluated properties uniform block, and how far the
    /// placement's fade has come (`DrawFrame::placement_fade`). The colors are
    /// premultiplied r, g, b, a floats.
    pub struct SymbolEvaluatedPropsUBO {
        pub text_fill_color: [f32; 4],
        pub text_halo_color: [f32; 4],
        pub text_opacity: f32,
        pub text_halo_width: f32,
        pub text_halo_blur: f32,
        pub pad1: f32,
        pub icon_fill_color: [f32; 4],
        pub icon_halo_color: [f32; 4],
        pub icon_opacity: f32,
        pub icon_halo_width: f32,
        pub icon_halo_blur: f32,
        pub fade_change: f32,
    }
}
const _: () = assert!(size_of::<SymbolEvaluatedPropsUBO>() == 6 * 16);

mln_gfx::uniform_block! {
    pub struct CollisionDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}
const _: () = assert!(size_of::<CollisionDrawableUBO>() == 4 * 16);

mln_gfx::uniform_block! {
    pub struct CollisionTilePropsUBO {
        pub extrude_scale: [f32; 2],
        pub overscale_factor: f32,
        pub pad1: f32,
    }
}
const _: () = assert!(size_of::<CollisionTilePropsUBO>() == 16);
