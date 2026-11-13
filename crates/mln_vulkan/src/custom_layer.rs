// What a custom layer's host gets of a Vulkan frame (C++:
// `style::vulkan::CustomLayerRenderParameters`, which also has the device's
// dispatcher; here the device's raw handles load its functions).

use ash::vk;

use crate::device::RawDevice;

/// The frame's command buffer outside the map's render pass, for a custom
/// layer's prerender.
///
/// Record work such as copies, compute dispatches and render passes of your
/// own, with the barriers that it needs; the map's render pass doesn't wait
/// for writes to your images. Don't end or submit `command_buffer`, and don't
/// submit to `device.queue` from a callback.
#[derive(Clone, Copy, Debug)]
pub struct CustomLayerEncoder {
    /// The device that the session renders on, to make objects with.
    pub device: RawDevice,
    /// Recording outside any render pass.
    pub command_buffer: vk::CommandBuffer,
}

/// The map's render pass at a custom layer's place in the style.
///
/// Record draws into `command_buffer` with pipelines made for `render_pass`,
/// `subpass` and `samples`, binding your own pipeline, descriptor sets,
/// buffers and push constants, and setting the dynamic state your pipelines
/// have. The map binds and sets again what its next draws need; your pipeline
/// layouts can differ from the map's. Record nothing that a render pass can't
/// hold, and don't end the render pass.
#[derive(Clone, Copy, Debug)]
pub struct CustomLayerPass {
    /// The device that the session renders on, to make objects with.
    pub device: RawDevice,
    pub command_buffer: vk::CommandBuffer,
    pub render_pass: vk::RenderPass,
    pub subpass: u32,
    pub samples: vk::SampleCountFlags,
    pub color_format: vk::Format,
    /// `UNDEFINED` when the pass has no depth-stencil attachment.
    pub depth_stencil_format: vk::Format,
    /// The size of the pass's attachments.
    pub extent: vk::Extent2D,
}
