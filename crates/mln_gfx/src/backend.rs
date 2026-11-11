// Ports include/mln/gfx/backend.hpp and the `Backend::enableGPUExpressionEval`
// definition from platform/default/src/mln/gfx/headless_backend.cpp.
//
// `Backend::Create<Type>()` is a C++ template specialised per backend; in the
// port the backend crate's constructor is called directly (for example
// `mln_webgpu::HeadlessBackend::new`). C++ builds one backend; the port can
// build several (WebGPU, Metal, OpenGL), and the renderer asks a context
// which one it is of (`Context::backend_type`).

use std::sync::atomic::{AtomicBool, Ordering};

/// `gfx::Backend::Type`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    OpenGL,
    Metal,
    Vulkan,
    WebGPU,
}

static ENABLE_GPU_EXPRESSION_EVAL: AtomicBool = AtomicBool::new(false);

pub struct Backend;

impl Backend {
    /// The backend on every platform (`MLN_RENDER_BACKEND_WEBGPU`).
    pub const DEFAULT_TYPE: Type = Type::WebGPU;

    pub fn get_type() -> Type {
        Self::DEFAULT_TYPE
    }

    pub fn get_enable_gpu_expression_eval() -> bool {
        ENABLE_GPU_EXPRESSION_EVAL.load(Ordering::Relaxed)
    }

    pub fn set_enable_gpu_expression_eval(value: bool) {
        ENABLE_GPU_EXPRESSION_EVAL.store(value, Ordering::Relaxed);
    }
}
