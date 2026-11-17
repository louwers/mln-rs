use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLTexture};

pub(crate) type MTLDevicePtr = Retained<ProtocolObject<dyn MTLDevice>>;
pub(crate) type MTLTexturePtr = Retained<ProtocolObject<dyn MTLTexture>>;
