// Ports include/mln/gfx/backend.hpp
//
// C++ builds one backend; the port can build several (WebGPU, Metal, Vulkan,
// OpenGL), and the renderer asks a context which one it is of
// (`Context::backend_type`).

/// `gfx::Backend::Type`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    OpenGL,
    Metal,
    Vulkan,
    WebGPU,
}
