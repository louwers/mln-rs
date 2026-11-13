// Compiles every GLSL source to SPIR-V in both projection variants, and
// checks the bindings each one declares against the descriptor set layouts.
mod glsl_compile_test;
// The window backend on a headless surface (VK_EXT_headless_surface): frames
// through the swapchain, resizes, the overlay, readback, two frames in
// flight.
mod surface_test;
// The background shader end to end on the drawable path (after
// crates/mln_webgpu/tests/background_test.rs): an offscreen target, a
// DrawableBuilder drawable in a TileLayerGroup, tile clipping masks,
// rendered into the offscreen target and read back.
mod background_test;
