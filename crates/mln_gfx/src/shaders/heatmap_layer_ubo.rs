use bytemuck::{Pod, Zeroable};

/// The heatmap shader's per-drawable uniform block (80 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HeatmapDrawableUBO {
    /*  0 */ pub matrix: [f32; 4 * 4],
    /* 64 */ pub extrude_scale: f32,

    // Interpolations
    /* 68 */ pub weight_t: f32,
    /* 72 */ pub radius_t: f32,
    /* 76 */ pub globe_extrude_scale: f32,
    /* 80 */
}
const _: () = assert!(std::mem::size_of::<HeatmapDrawableUBO>() == 5 * 16);

/// Evaluated properties that do not depend on the tile (16 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct HeatmapEvaluatedPropsUBO {
    /*  0 */ pub weight: f32,
    /*  4 */ pub radius: f32,
    /*  8 */ pub intensity: f32,
    /* 12 */ pub padding: f32,
    /* 16 */
}
const _: () = assert!(std::mem::size_of::<HeatmapEvaluatedPropsUBO>() == 16);
