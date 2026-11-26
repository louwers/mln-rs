// Compiles and links every GLSL program, in both projection variants, and
// checks its uniform blocks against the UBOs the renderer writes.
mod glsl_compile_test;
// mln_engine::conformance's frame through `GlGpu`, and the draws that
// `GlGpu` rejects.
#[cfg(not(target_arch = "wasm32"))]
mod gpu_test;
// A borrowed ring's imports and framebuffers across frames.
#[cfg(not(target_arch = "wasm32"))]
mod borrowed_target_test;
