//! The tests of the layer types' buckets, layouts and meshes.

// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// The custom layer's host, as its layer keeps it.
mod custom_test;
mod fill_mesh_test;
// Ports test/layout/sort_key.test.cpp (fill, line, circle).
mod sort_key_test;
// The fill-extrusion parts of the subdivision tests.
mod subdivision_test;

/// Runs `f` with the `Gpu` of a headless WebGPU backend, which compiles the
/// built-in layer types' programs, and a context over it, which it then
/// tears down.
fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut mln_renderer::unstable::Context)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let device = mln_webgpu::Device::new(None).expect("device");
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    let mut sources = mln_gfx::shaders::source::ShaderSources::default();
    sources.add(mln_webgpu::LAYER_SOURCES);
    gpu.add_shader_sources(&sources);
    let mut context = mln_renderer::unstable::Context::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}

/// The built-in expression functions and the layer types of this crate, as
/// this build has them: the tests' layers must be of its types, not of
/// those of the copy of this crate that the facade registers.
fn registry() -> &'static std::sync::Arc<mln_renderer::Registry> {
    static REGISTRY: std::sync::LazyLock<std::sync::Arc<mln_renderer::Registry>> = std::sync::LazyLock::new(|| {
        let mut builder = mln_renderer::RegistryBuilder::new();
        mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
        crate::register_layer_types(&mut builder).unwrap();
        builder.build()
    });
    &REGISTRY
}
