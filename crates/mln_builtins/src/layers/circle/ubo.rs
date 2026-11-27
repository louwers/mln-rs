//! The circles' blocks.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// The circle shader's per-drawable uniform block.
    pub struct CircleDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub extrude_scale: [f32; 2],

        // Interpolations
        pub color_t: f32,
        pub radius_t: f32,
        pub blur_t: f32,
        pub opacity_t: f32,
        pub stroke_color_t: f32,
        pub stroke_width_t: f32,
        pub stroke_opacity_t: f32,
        pub globe_extrude_scale: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile. The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct CircleEvaluatedPropsUBO {
        pub color: [f32; 4],
        pub stroke_color: [f32; 4],
        pub radius: f32,
        pub blur: f32,
        pub opacity: f32,
        pub stroke_width: f32,
        pub stroke_opacity: f32,
        pub scale_with_map: i32,
        pub pitch_with_map: i32,
        pub pad1: f32,
    }
}
