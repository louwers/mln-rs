use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandQueue, MTLDepthStencilState, MTLDevice, MTLFunction,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState, MTLSamplerState, MTLTexture,
    MTLVertexDescriptor,
};

pub(crate) type MTLBufferPtr = Retained<ProtocolObject<dyn MTLBuffer>>;
pub(crate) type MTLCommandBufferPtr = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
pub(crate) type MTLCommandQueuePtr = Retained<ProtocolObject<dyn MTLCommandQueue>>;
pub(crate) type MTLDevicePtr = Retained<ProtocolObject<dyn MTLDevice>>;
pub(crate) type MTLDepthStencilStatePtr = Retained<ProtocolObject<dyn MTLDepthStencilState>>;
pub(crate) type MTLFunctionPtr = Retained<ProtocolObject<dyn MTLFunction>>;
pub(crate) type MTLRenderCommandEncoderPtr = Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>;
pub(crate) type MTLRenderPassDescriptorPtr = Retained<MTLRenderPassDescriptor>;
pub(crate) type MTLRenderPipelineStatePtr = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
pub(crate) type MTLSamplerStatePtr = Retained<ProtocolObject<dyn MTLSamplerState>>;
pub(crate) type MTLTexturePtr = Retained<ProtocolObject<dyn MTLTexture>>;
pub(crate) type MTLVertexDescriptorPtr = Retained<MTLVertexDescriptor>;
