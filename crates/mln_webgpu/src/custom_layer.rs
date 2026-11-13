// What a custom layer's host gets of a WebGPU frame. The pass and the
// encoder are the frame's own wgpu objects, moved to the host for the call
// and back after it.

/// The frame's command encoder outside the map's render pass, for a custom
/// layer's prerender. End each pass begun on `encoder` before the call
/// returns, and leave the encoder in place.
pub struct CustomLayerEncoder {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub encoder: wgpu::CommandEncoder,
}

/// The map's render pass at a custom layer's place in the style.
///
/// Set your own pipeline, bind groups, buffers, viewport, scissor rectangle
/// and stencil reference on `pass`: the map sets what its next draws need
/// again. Leave `pass` in place.
pub struct CustomLayerPass {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub pass: wgpu::RenderPass<'static>,
    pub color_format: wgpu::TextureFormat,
    /// None when the pass has no depth-stencil attachment.
    pub depth_stencil_format: Option<wgpu::TextureFormat>,
    pub sample_count: u32,
}
