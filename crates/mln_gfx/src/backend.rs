// C++ builds one backend; the port can build several (WebGPU, Metal, Vulkan,
// OpenGL), and the renderer asks the backend's `Gpu` which one it is
// (`Gpu::backend_type`).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    OpenGL,
    Metal,
    Vulkan,
    WebGPU,
}
