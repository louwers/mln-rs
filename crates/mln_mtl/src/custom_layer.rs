// What a custom layer's host gets of a Metal frame.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLPixelFormat, MTLRenderCommandEncoder};

/// The pixel formats of a pass's attachments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttachmentFormats {
    pub color: MTLPixelFormat,
    pub depth: Option<MTLPixelFormat>,
    pub stencil: Option<MTLPixelFormat>,
}

/// The frame's command buffer, with no encoder open, for a custom layer's
/// prerender. End each encoder made on it before the call returns, and don't
/// commit it.
pub struct CustomLayerEncoder {
    pub command_buffer: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
}

/// The map's render pass at a custom layer's place in the style.
///
/// Set your own pipeline state, depth-stencil state, buffers, textures and
/// samplers on `encoder`, and any other state your draws need: the map sets
/// what its next draws need again. Don't end `encoder`, and don't commit
/// `command_buffer`.
pub struct CustomLayerPass {
    pub encoder: Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>,
    pub command_buffer: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    pub formats: AttachmentFormats,
    pub sample_count: usize,
}
