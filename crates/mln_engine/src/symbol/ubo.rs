//! The symbols' blocks.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The symbol shader's per-drawable uniform block.
    pub struct SymbolDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub label_plane_matrix: [f32; 4 * 4],
        pub coord_matrix: [f32; 4 * 4],
        pub texsize: [f32; 2],
        pub texsize_icon: [f32; 2],
        pub is_text_prop: i32,
        pub rotate_symbol: i32,
        pub pitch_with_map: i32,
        pub is_size_zoom_constant: i32,
        pub is_size_feature_constant: i32,
        pub is_offset: i32,
        pub size_t: f32,
        pub size: f32,
        // Interpolations
        pub fill_color_t: f32,
        pub halo_color_t: f32,
        pub opacity_t: f32,
        pub halo_width_t: f32,
        pub halo_blur_t: f32,
        pub is_along_line: i32,
        pub is_variable_anchor: i32,
        pub pitched_scale: f32,
        pub translation: [f32; 2],
        pub gamma_scale: f32,
        pub is_halo: i32,
    }
}

uniform_block! {
    pub struct SymbolTilePropsUBO {
        pub is_text: i32,
        pub is_halo: i32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// The symbol shader's evaluated properties uniform block. The colors are
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
        pub pad2: f32,
    }
}
