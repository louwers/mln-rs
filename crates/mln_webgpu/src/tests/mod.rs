// Compiles all extracted WGSL with wgpu (naga); the M1 shaders must compile.
mod wgsl_compile_test;
// Ports test/renderer/wgsl_preprocessor.test.cpp
mod shader_group_preprocessor;
// HeadlessBackend on a shared or host-made device, and on a named adapter.
mod shared_device_test;
// The background shader end to end on the drawable path: HeadlessBackend,
// BackgroundShader from the registry, a DrawableBuilder drawable in a
// TileLayerGroup with the UBOs background_layer_tweaker.cpp sets, rendered
// into the offscreen target and read back.
mod background_test;
