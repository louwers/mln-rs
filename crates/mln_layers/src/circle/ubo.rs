use mln_gfx::shaders::block::Bool32;

mln_gfx::uniform_block! {
    /// The circle shader's per-drawable uniform block.
    pub struct CircleDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub extrude_scale: [f32; 2],
        // Interpolations
        #[glsl_es(lowp)]
        pub color_t: f32,
        #[glsl_es(lowp)]
        pub radius_t: f32,
        #[glsl_es(lowp)]
        pub blur_t: f32,
        #[glsl_es(lowp)]
        pub opacity_t: f32,
        #[glsl_es(lowp)]
        pub stroke_color_t: f32,
        #[glsl_es(lowp)]
        pub stroke_width_t: f32,
        #[glsl_es(lowp)]
        pub stroke_opacity_t: f32,
        pub globe_extrude_scale: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile. The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct CircleEvaluatedPropsUBO {
        pub color: [f32; 4],
        pub stroke_color: [f32; 4],
        #[glsl_es(mediump)]
        pub radius: f32,
        #[glsl_es(lowp)]
        pub blur: f32,
        #[glsl_es(lowp)]
        pub opacity: f32,
        #[glsl_es(mediump)]
        pub stroke_width: f32,
        #[glsl_es(lowp)]
        pub stroke_opacity: f32,
        pub scale_with_map: Bool32,
        pub pitch_with_map: Bool32,
        #[padding]
        pub pad1: f32,
    }
}
