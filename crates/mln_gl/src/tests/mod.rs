// Compiles and links every GLSL program, in both projection variants, and
// checks its uniform blocks against the UBOs the renderer writes.
mod glsl_compile_test;
// mln_gfx::gpu::conformance's frame through `GlGpu`, and the draws that
// `GlGpu` rejects.
#[cfg(not(target_arch = "wasm32"))]
mod gpu_test;
