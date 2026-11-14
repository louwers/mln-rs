// Compiles every GLSL source to SPIR-V in both projection variants, and
// checks the bindings each one declares against the descriptor set layouts.
mod glsl_compile_test;
// The window backend on a headless surface (VK_EXT_headless_surface): frames
// through the swapchain, resizes, the overlay, readback, two frames in
// flight.
mod surface_test;
// mln_gfx::gpu::conformance's frame through `VulkanGpu`, and its shaders
// and texture writes.
mod gpu_test;
