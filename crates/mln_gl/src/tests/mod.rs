// A drawable's vertex arrays follow its index vector to a new buffer, though
// the new buffer may have the GL name of the one it replaced.
mod drawable_test;
// Compiles and links every GLSL program, in both projection variants, and
// checks its uniform blocks against the UBOs the renderer writes.
mod glsl_compile_test;
// The background shader end to end on the drawable path (after
// crates/mln_webgpu/tests/background_test.rs): an offscreen target, a
// DrawableBuilder drawable in a TileLayerGroup, tile clipping masks,
// rendered into the offscreen target and read back.
mod background_test;
