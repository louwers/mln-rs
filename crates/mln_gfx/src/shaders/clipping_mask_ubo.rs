// Lives in mln_gfx because the renderer builds the UBOs
// (`PaintParameters::renderTileClippingMasks`, paint_parameters.cpp) and
// passes them to `Context::render_tile_clipping_masks`.

use bytemuck::{Pod, Zeroable};

/// The clipping mask uniform block (80 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct ClipUBO {
    pub matrix: [f32; 4 * 4],
    pub stencil_ref: u32,
    pub pad1: f32,
    pub pad2: f32,
    pub pad3: f32,
}
const _: () = assert!(size_of::<ClipUBO>() == 5 * 16);
