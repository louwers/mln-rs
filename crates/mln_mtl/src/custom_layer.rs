// What a custom layer's host gets of a Metal frame (C++:
// `style::mtl::CustomLayerRenderParameters`).

use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderCommandEncoderPtr};
use crate::render_pass::AttachmentFormats;

/// The frame's command buffer, with no encoder open, for a custom layer's
/// prerender. End each encoder made on it before the call returns, and don't
/// commit it.
pub struct CustomLayerEncoder {
    pub command_buffer: MTLCommandBufferPtr,
}

/// The map's render pass at a custom layer's place in the style.
///
/// Set your own pipeline state, depth-stencil state, buffers, textures and
/// samplers on `encoder`, and any other state your draws need: the map sets
/// what its next draws need again. Don't end `encoder`, and don't commit
/// `command_buffer`.
pub struct CustomLayerPass {
    pub encoder: MTLRenderCommandEncoderPtr,
    pub command_buffer: MTLCommandBufferPtr,
    pub formats: AttachmentFormats,
    pub sample_count: usize,
}
