use mln_gfx::shaders::block::Bool32;

mln_gfx::uniform_block! {
    /// A circle draw's block: how far a pixel reaches, in the tile or in
    /// clip space, the factor that mixes each property's zoom stops, which
    /// the engine writes, and the radians a pixel spans on the globe.
    pub struct CircleDrawableUBO {
        pub extrude_scale: [f32; 2],
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
    /// The layer's block: each property's value where it is the same for
    /// every feature, which the engine writes, and the alignments.
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
