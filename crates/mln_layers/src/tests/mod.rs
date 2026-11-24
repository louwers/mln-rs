//! The tests of the layer types' buckets, layouts and meshes.

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

/// The update of a continuous map of `style` at pixel ratio 1, of
/// `width` × `height` pixels centered on 0, 0 at `zoom`, as a frame of the
/// benchmarks' entry points takes it.
pub(crate) fn update(style: &mln_style::Style, zoom: f64, (width, height): (u32, u32)) -> mln_render::UpdateParameters {
    use std::sync::Arc;

    use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
    use mln_util::geo::LatLng;
    use mln_util::size::Size;
    use mln_util::time::Instant;

    let mut state = TransformState::default();
    state.set_size(Size::new(width, height));
    state.set_lat_lng_zoom(&LatLng::new(0.0, 0.0).unwrap(), zoom);
    mln_render::UpdateParameters {
        style_loaded: true,
        mode: MapMode::Continuous,
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        time_point: Some(Instant::now()),
        view: state.view().clone(),
        min_zoom: state.min_zoom(),
        max_zoom: state.limits().max_zoom,
        camera_moving: false,
        style: Arc::new(style.clone()),
        sprite_loaded: true,
        feature_states: Arc::default(),
        file_source: None,
        prefetch_zoom_delta: 0,
        still_image_request: false,
        still_image_request_id: 0,
        update_id: 0,
        cross_source_collisions: true,
        fast_pfor_enabled: false,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: style.get_projection().get_subdivision_granularity(),
    }
}
