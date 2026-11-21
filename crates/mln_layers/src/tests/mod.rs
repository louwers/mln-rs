//! The tests of the layer types' buckets, layouts and meshes.

// Ports test/gl/bucket.test.cpp (circle, fill, line, symbol; on the WebGPU
// headless backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// The custom layer's host, as its layer keeps it.
mod custom_test;
mod fill_mesh_test;
// The symbol layer type's format sections that override its paint.
mod format_section_override_test;
// The globe meshes that the raster layer types keep.
mod globe_mesh_test;
// The subdivision of polygons for the globe.
mod polygon_subdivision_test;
// Ports test/layout/sort_key.test.cpp.
mod sort_key_test;
// The fill-extrusion parts of the subdivision tests.
mod subdivision_test;

/// Runs `f` with the `Gpu` of a headless WebGPU backend, which compiles the
/// built-in layer types' programs, and a context over it, which it then
/// tears down.
fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut mln_render::draw::Context)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let device = mln_webgpu::Device::new(None).expect("device");
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    gpu.add_shader_sources(&crate::shader_sources());
    let mut context = mln_render::draw::Context::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}

/// The built-in expression functions and the layer types of this crate, as
/// this build has them: the tests' layers must be of its types, not of
/// those of the copy of this crate that the facade registers.
pub(crate) fn registry() -> &'static std::sync::Arc<mln_render::Registry> {
    static REGISTRY: std::sync::LazyLock<std::sync::Arc<mln_render::Registry>> = std::sync::LazyLock::new(|| {
        let mut builder = mln_render::RegistryBuilder::new();
        mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
        crate::register_layer_types(&mut builder).unwrap();
        builder.build()
    });
    &REGISTRY
}
