//! The tests of the layer types' buckets, layouts and meshes.

// The custom layer's host, as its layer keeps it.
mod custom_test;
// The meshes of fills and their outlines.
mod fill_mesh_test;
// The symbol layer type's format sections that override its paint.
mod format_section_override_test;
// The subdivision of polygons for the globe.
mod polygon_subdivision_test;
// The programs' declarations and sources.
mod programs_test;
// The order of symbols with sort keys.
mod sort_key_test;
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

/// The built-in expression functions and source types, and the layer types
/// of this crate, as this build has them: the tests' layers must be of its
/// types, not of those of the copy of this crate that the facade registers.
pub(crate) fn registry() -> &'static std::sync::Arc<mln_render::Registry> {
    static REGISTRY: std::sync::LazyLock<std::sync::Arc<mln_render::Registry>> = std::sync::LazyLock::new(|| {
        let mut builder = mln_render::RegistryBuilder::new();
        mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
        mln_sources::register_source_types(&mut builder).unwrap();
        crate::register_layer_types(&mut builder).unwrap();
        builder.build()
    });
    &REGISTRY
}
