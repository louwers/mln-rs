// Ports include/mln/mtl/mtl_fwd.hpp: the owning pointers to Metal objects.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandQueue, MTLDepthStencilState, MTLDevice, MTLFunction,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState, MTLSamplerState, MTLTexture,
    MTLVertexDescriptor,
};

pub type MTLBufferPtr = Retained<ProtocolObject<dyn MTLBuffer>>;
pub type MTLCommandBufferPtr = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
pub type MTLCommandQueuePtr = Retained<ProtocolObject<dyn MTLCommandQueue>>;
pub type MTLDevicePtr = Retained<ProtocolObject<dyn MTLDevice>>;
pub type MTLDepthStencilStatePtr = Retained<ProtocolObject<dyn MTLDepthStencilState>>;
pub type MTLFunctionPtr = Retained<ProtocolObject<dyn MTLFunction>>;
pub type MTLRenderCommandEncoderPtr = Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>;
pub type MTLRenderPassDescriptorPtr = Retained<MTLRenderPassDescriptor>;
pub type MTLRenderPipelineStatePtr = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
pub type MTLSamplerStatePtr = Retained<ProtocolObject<dyn MTLSamplerState>>;
pub type MTLTexturePtr = Retained<ProtocolObject<dyn MTLTexture>>;
pub type MTLVertexDescriptorPtr = Retained<MTLVertexDescriptor>;
