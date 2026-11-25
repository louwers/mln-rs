//! The fill extrusions' blocks.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The fill extrusion shader's per-drawable uniform block.
    pub struct FillExtrusionDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub pixel_coord_upper: [f32; 2],
        pub pixel_coord_lower: [f32; 2],
        pub height_factor: f32,
        pub tile_ratio: f32,

        // Interpolations
        pub base_t: f32,
        pub height_t: f32,
        pub color_t: f32,
        pub pattern_from_t: f32,
        pub pattern_to_t: f32,
        /// Where `FillExtrusionPulledShader` finds a vertex's paint values in
        /// the buffer it reads them from, in words, a byte each from the
        /// lowest: how many a vertex takes, then the offset in them of its
        /// base, its height and its color.
        pub paint_layout: u32,
    }
}

uniform_block! {
    /// The fill extrusion shader's per-tile uniform block.
    pub struct FillExtrusionTilePropsUBO {
        pub pattern_from: [f32; 4],
        pub pattern_to: [f32; 4],
        pub texsize: [f32; 2],
        pub pad1: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile. `color`
    /// is the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct FillExtrusionPropsUBO {
        pub color: [f32; 4],
        pub light_color: [f32; 3],
        pub pad1: f32,
        pub light_position: [f32; 3],
        pub base: f32,
        pub height: f32,
        pub light_intensity: f32,
        pub vertical_gradient: f32,
        pub opacity: f32,
        pub fade: f32,
        pub from_scale: f32,
        pub to_scale: f32,
        pub pad2: f32,
    }
}
