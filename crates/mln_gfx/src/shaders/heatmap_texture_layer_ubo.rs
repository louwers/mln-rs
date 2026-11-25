use crate::shaders::block::uniform_block;

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
