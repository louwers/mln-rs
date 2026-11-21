mln_gfx::uniform_block! {
    /// The heatmap shader's per-drawable uniform block.
    pub struct HeatmapDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub extrude_scale: f32,
        // Interpolations
        #[glsl_es(lowp)]
        pub weight_t: f32,
        #[glsl_es(lowp)]
        pub radius_t: f32,
        pub globe_extrude_scale: f32,
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct HeatmapEvaluatedPropsUBO {
        pub weight: f32,
        pub radius: f32,
        pub intensity: f32,
        #[padding]
        pub padding: f32,
    }
}

mln_gfx::uniform_block! {
    /// The heatmap texture shader's properties uniform block.
    pub struct HeatmapTexturePropsUBO {
        pub matrix: [f32; 4 * 4],
        pub opacity: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}
