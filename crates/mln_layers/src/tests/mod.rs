//! The tests of the layer types' buckets, layouts and meshes.

// Ports test/gl/bucket.test.cpp (circle, fill, line, symbol; on the WebGPU
// headless backend) and test/renderer/fill_bucket.test.cpp.
// slice: off mod bucket_test;
// The custom layer's host, as its layer keeps it.
// slice: off mod custom_test;
// slice: off mod fill_mesh_test;
// The symbol layer type's format sections that override its paint.
mod format_section_override_test;
// The globe meshes that the raster layer types keep.
// slice: off mod globe_mesh_test;
// The subdivision of polygons for the globe.
// slice: off mod polygon_subdivision_test;
// The programs' declarations and sources.
mod programs_test;
// Ports test/layout/sort_key.test.cpp.
// slice: off mod sort_key_test;
// The fill-extrusion parts of the subdivision tests.
mod subdivision_test;

/// Subdivides `polygon` for the globe, filled with earcut's triangles.
fn subdivide(
    polygon: &mln_tile_id::geometry_tile_data::GeometryCollection,
    tile: &mln_tile_id::CanonicalTileID,
    granularity: u32,
    outline: bool,
) -> mln_render::globe::SubdividedPolygon {
    let mut tessellator = crate::tessellator::Tessellator::default();
    let triangles = tessellator.triangulate(polygon);
    mln_render::globe::subdivide_polygon(polygon, triangles, tile, granularity, outline)
}
// slice: off
// slice: off /// Runs `f` with the `Gpu` of a headless WebGPU backend, which compiles the
// slice: off /// built-in layer types' programs, and a context over it, which it then
// slice: off /// tears down.
// slice: off fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut mln_render::draw::Context)) {
// slice: off     use mln_gfx::RendererBackend as _;
// slice: off     let device = mln_webgpu::Device::new(None).expect("device");
// slice: off     let mut backend = mln_webgpu::RendererBackend::new(&device);
// slice: off     let gpu = backend.gpu();
// slice: off     mln_render::testing::with_context(gpu, 1.0, f);
// slice: off }
// slice: off
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
