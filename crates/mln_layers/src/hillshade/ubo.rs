/// Maximum number of illumination sources supported
pub const MAX_ILLUMINATION_SOURCES: usize = 4;

mln_gfx::uniform_block! {
    /// The hillshade shader's per-drawable uniform block (64 bytes).
    pub struct HillshadeDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}
const _: () = assert!(size_of::<HillshadeDrawableUBO>() == 4 * 16);

mln_gfx::uniform_block! {
    /// The hillshade shader's per-tile uniform block (32 bytes).
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
const _: () = assert!(size_of::<HillshadeTilePropsUBO>() == 32);

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile (176 bytes). The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct HillshadeEvaluatedPropsUBO {
        pub accent: [f32; 4],
        pub altitudes: [f32; 4],
        pub azimuths: [f32; 4],
        pub shadows: [[f32; 4]; 4],
        pub highlights: [[f32; 4]; 4],
    }
}
const _: () = assert!(size_of::<HillshadeEvaluatedPropsUBO>() == 176);

mln_gfx::uniform_block! {
    /// The hillshade prepare shader's per-drawable uniform block (64 bytes).
    pub struct HillshadePrepareDrawableUBO {
        pub matrix: [f32; 4 * 4],
    }
}
const _: () = assert!(size_of::<HillshadePrepareDrawableUBO>() == 4 * 16);

mln_gfx::uniform_block! {
    /// The hillshade prepare shader's per-tile uniform block (32 bytes).
    pub struct HillshadePrepareTilePropsUBO {
        pub unpack: [f32; 4],
        pub dimension: [f32; 2],
        pub zoom: f32,
        pub maxzoom: f32,
    }
}
const _: () = assert!(size_of::<HillshadePrepareTilePropsUBO>() == 2 * 16);
