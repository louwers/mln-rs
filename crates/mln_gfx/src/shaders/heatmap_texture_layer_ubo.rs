use bytemuck::{Pod, Zeroable};

/// The heatmap texture shader's properties uniform block (80 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HeatmapTexturePropsUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub opacity: f32,
    /* 68 */ pub pad1: f32,
    /* 72 */ pub pad2: f32,
    /* 76 */ pub pad3: f32,
    /* 80 */
}
const _: () = assert!(size_of::<HeatmapTexturePropsUBO>() == 5 * 16);
