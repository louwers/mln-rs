//! Tests of the layer types' buckets, layouts and meshes.

// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
mod fixture_log;
mod gtest_fill_mesh;
mod gtest_subdivision;
// Ports test/layout/sort_key.test.cpp.
mod sort_key_test;

/// Runs `f` with the `Gpu` of a headless WebGPU backend and a context over
/// it, which it then tears down. Skipped where no WebGPU adapter is
/// available unless `MLN_RENDER_BACKEND` asks for `webgpu`.
fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut mln_engine::draw::Context)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let requested = std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("webgpu"));
    let device = match mln_webgpu::Device::new(None) {
        Ok(device) => device,
        Err(error) if error.kind == mln_gfx::ErrorKind::Unsupported && !requested => {
            eprintln!("skipped: {error}");
            return;
        }
        Err(error) => panic!("WebGPU device: {error}"),
    };
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    let mut context = mln_engine::draw::Context::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}
