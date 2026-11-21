mln_gfx::uniform_block! {
    /// The heatmap shader's per-drawable uniform block (80 bytes).
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
const _: () = assert!(size_of::<HeatmapDrawableUBO>() == 5 * 16);

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile (16 bytes).
    pub struct HeatmapEvaluatedPropsUBO {
        pub weight: f32,
        pub radius: f32,
        pub intensity: f32,
        pub padding: f32,
    }
}
const _: () = assert!(size_of::<HeatmapEvaluatedPropsUBO>() == 16);

mln_gfx::uniform_block! {
    /// The heatmap texture shader's properties uniform block (80 bytes).
    pub struct HeatmapTexturePropsUBO {
        pub matrix: [f32; 4 * 4],
        pub opacity: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}
const _: () = assert!(size_of::<HeatmapTexturePropsUBO>() == 5 * 16);
