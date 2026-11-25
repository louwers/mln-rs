//! The heatmaps' blocks, and the heatmap texture's.

use mln_gfx::shaders::block::uniform_block;

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

uniform_block! {
    /// The heatmap texture shader's properties uniform block.
    pub struct HeatmapTexturePropsUBO {
        pub matrix: [f32; 4 * 4],
        pub opacity: f32,
        pub pad1: f32,
        pub pad2: f32,
        pub pad3: f32,
    }
}
