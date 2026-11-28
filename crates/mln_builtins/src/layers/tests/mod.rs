//! Tests of the layer types' buckets, layouts and meshes.

// Ports test/gl/bucket.test.cpp (circle, fill, line, symbol; on the WebGPU
// headless backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
mod calculate_tile_distances;
mod fixture_log;
mod globe_meshes_test;
mod gtest_fill_mesh;
mod gtest_subdivision;
mod raster_quads_test;
// Ports test/layout/sort_key.test.cpp.
mod sort_key_test;
mod terrain_labels_test;

/// Runs `f` with resources over the `Gpu` of a headless WebGPU backend,
/// which it then tears down. Skipped where no WebGPU adapter is available
/// unless `MLN_RENDER_BACKEND` asks for `webgpu`.
fn with_resources(f: impl FnOnce(&mut mln_engine::draw::Resources<'_>)) {
    use mln_gfx::RendererBackend as _;
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
    mln_engine::draw::Resources::with_test_resources(backend.gpu(), f);
}
