use crate::shaders::block::uniform_block;

/// Maximum number of illumination sources supported
pub const MAX_ILLUMINATION_SOURCES: usize = 4;

uniform_block! {
    /// The hillshade shader's per-drawable uniform block.
    pub struct HillshadeDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}

uniform_block! {
    /// The hillshade shader's per-tile uniform block.
    pub struct HillshadeTilePropsUBO {
        pub latrange: [f32; 2],
        pub exaggeration: f32,
        pub method: i32,
        pub num_lights: i32,
        pub pad0: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile. The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct HillshadeEvaluatedPropsUBO {
        pub accent: [f32; 4],
        pub altitudes: [f32; 4],
        pub azimuths: [f32; 4],
        pub shadows: [[f32; 4]; MAX_ILLUMINATION_SOURCES],
        pub highlights: [[f32; 4]; MAX_ILLUMINATION_SOURCES],
    }
}
