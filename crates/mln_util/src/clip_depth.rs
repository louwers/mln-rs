/// How a graphics API's clip space holds depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipDepthRange {
    /// Clip z from 0 to w: Metal, Vulkan, D3D12 and WebGPU.
    ZeroToOne,
    /// Clip z from -w to w: OpenGL.
    NegativeOneToOne,
}
