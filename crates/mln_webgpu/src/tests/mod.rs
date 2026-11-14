// Compiles all extracted WGSL with wgpu (naga); the M1 shaders must compile.
mod wgsl_compile_test;
// Ports test/renderer/wgsl_preprocessor.test.cpp
mod shader_group_preprocessor;
// HeadlessBackend on a shared or host-made device, and on a named adapter.
mod shared_device_test;
// mln_gfx::gpu::conformance's frame through `WebGpu`.
mod gpu_test;
