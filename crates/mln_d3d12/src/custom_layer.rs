// What a custom layer's host gets of a D3D12 frame.

use windows::Win32::Graphics::Direct3D12::{D3D12_CPU_DESCRIPTOR_HANDLE, ID3D12Device, ID3D12GraphicsCommandList};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The frame's command list outside the map's passes, for a custom layer's
/// prerender.
///
/// Record work such as copies and passes of your own, with the barriers
/// that it needs, and leave the resources you use in the states you found
/// them in. Don't close or execute `command_list`. The map sets its own
/// descriptor heaps and root signature again after the call.
pub struct CustomLayerEncoder {
    /// The device that the session renders on, to make objects with.
    pub device: ID3D12Device,
    pub command_list: ID3D12GraphicsCommandList,
}

/// The map's pass at a custom layer's place in the style: its render
/// targets are set on `command_list`.
///
/// Set your own pipeline state, root signature, descriptor heaps, vertex
/// and index buffers, topology, viewport, scissor rectangle and stencil
/// reference, with pipelines of `color_format` and `depth_stencil_format`.
/// The map sets what its next draws need again, its render targets
/// included.
pub struct CustomLayerPass {
    pub device: ID3D12Device,
    pub command_list: ID3D12GraphicsCommandList,
    /// The views of the pass's color and depth-stencil attachments.
    pub render_target: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub color_format: DXGI_FORMAT,
    /// `None` and `DXGI_FORMAT_UNKNOWN` when the pass has no depth-stencil
    /// attachment.
    pub depth_stencil: Option<D3D12_CPU_DESCRIPTOR_HANDLE>,
    pub depth_stencil_format: DXGI_FORMAT,
}
