use crate::shaders::block::uniform_block;

uniform_block! {
    /// The heatmap shader's per-drawable uniform block.
    pub struct HeatmapDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub extrude_scale: f32,

        // Interpolations
        pub weight_t: f32,
        pub radius_t: f32,
        pub globe_extrude_scale: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct HeatmapEvaluatedPropsUBO {
        pub weight: f32,
        pub radius: f32,
        pub intensity: f32,
        pub padding: f32,
    }
}
