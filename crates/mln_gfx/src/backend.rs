// The backends the port builds, which a host picks a device of. The
// renderer reads what it needs of one from its `Gpu`'s `Caps`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    OpenGL,
    Metal,
    Vulkan,
    WebGPU,
}
