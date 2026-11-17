// Compiles and links every program, and checks its uniform blocks' layout.
mod program_test;
// mln_gfx::gpu::conformance's frame through `GlGpu`, and the draws that
// `GlGpu` rejects.
#[cfg(not(target_arch = "wasm32"))]
mod gpu_test;
// A borrowed ring's imports and framebuffers across frames.
#[cfg(not(target_arch = "wasm32"))]
mod borrowed_target_test;
