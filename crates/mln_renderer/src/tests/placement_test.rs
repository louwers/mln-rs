use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, View};
use mln_style::Style;
use mln_style::expression::functions::RegisterError;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::time::Instant;

use crate::render_orchestrator::RenderOrchestrator;
use crate::tests::fixture_layer::{Dot, Mark, MarkPass};
use crate::{Registry, RegistryBuilder, UpdateParameters};

/// A registry of the layer types `dot` and `mark`, whose layers place.
fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
    builder.add_layer_type::<Dot>().unwrap();
    builder.add_layer_type::<Mark>().unwrap();
    builder.build()
}

fn orchestrator(registry: Arc<Registry>) -> RenderOrchestrator {
    RenderOrchestrator::new(
        true,
        mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
        Arc::default(),
        crate::text::font_face_engine::default_engine(),
        registry,
        Arc::new(|| {}),
        &crate::tests::pool(),
    )
}

/// An update in `mode` of a style of two `mark` layers.
fn update(registry: &Registry, mode: MapMode) -> Arc<UpdateParameters> {
    let json = r#"{"version": 8, "sources": {}, "layers": [{"id": "a", "type": "mark"}, {"id": "b", "type": "mark"}]}"#;
    let style = Style::from_json(registry.style(), json, None).unwrap();
    Arc::new(UpdateParameters {
        style_loaded: true,
        mode,
        pixel_ratio: 1.0,
        debug_options: MapDebugOptions::NO_DEBUG,
        time_point: (mode == MapMode::Continuous).then(Instant::now),
        view: View::default(),
        min_zoom: 0.0,
        max_zoom: 22.0,
        camera_moving: false,
        style: Arc::new(style),
        sprite_loaded: true,
        feature_states: Arc::default(),
        file_source: None,
        prefetch_zoom_delta: 0,
        still_image_request: mode != MapMode::Continuous,
        still_image_request_id: 1,
        cross_source_collisions: true,
        fast_pfor_enabled: false,
        tile_lod_min_radius: 3.0,
        tile_lod_scale: 1.0,
        tile_lod_pitch_threshold: mln_math::deg2rad(60.0),
        tile_lod_zoom_shift: 0.0,
        tile_lod_mode: TileLodMode::Default,
        subdivision_granularity: SubdivisionGranularitySetting::none(),
    })
}

/// The pass of `orchestrator`, as its type.
fn pass(orchestrator: &mut RenderOrchestrator) -> &mut MarkPass {
    orchestrator.placement_mut().unwrap().pass_mut().downcast_mut().unwrap()
}

/// The hooks of the pass that a frame in `mode` calls, by whether the pass
/// says its layers have something to place.
fn calls(mode: MapMode, places: bool) -> Vec<String> {
    let registry = registry();
    let mut orchestrator = orchestrator(Arc::clone(&registry));
    pass(&mut orchestrator).places = places;
    orchestrator.update(&update(&registry, mode)).expect("a frame");
    std::mem::take(&mut pass(&mut orchestrator).calls)
}

#[test]
fn a_continuous_frame_places_its_layers_then_brings_their_buckets_to_the_placement() {
    assert_eq!(calls(MapMode::Continuous, true), ["prepare 2", "place 2", "update_buckets 2"]);
}

#[test]
fn a_continuous_frame_with_nothing_to_place_resets_the_pass() {
    assert_eq!(calls(MapMode::Continuous, false), ["prepare 2", "reset", "update_buckets 2"]);
}

#[test]
fn a_still_image_places_and_keeps_nothing_of_the_buckets() {
    assert_eq!(calls(MapMode::Static, true), ["prepare 2", "place 2", "forget_buckets", "update_buckets 2"]);
    assert_eq!(calls(MapMode::Static, false), ["prepare 2", "forget_buckets", "update_buckets 2"]);
}

/// What `Renderer::placement` reads: the pass, as its type and no other.
#[test]
fn the_pass_is_reachable_as_its_type() {
    let placing = orchestrator(registry());
    let pass = placing.placement().unwrap().pass();
    assert!(pass.downcast_ref::<MarkPass>().is_some());
    assert!(pass.downcast_ref::<Dot>().is_none());
    assert!(orchestrator(Registry::with_builtins()).placement().is_none(), "no layer type places");
}

#[test]
fn a_registry_has_one_layer_type_that_places() {
    let mut builder = RegistryBuilder::new();
    builder.add_layer_type::<Mark>().unwrap();
    assert_eq!(builder.add_layer_type::<Mark<true>>(), Err(RegisterError::RoleTaken("other-mark".to_owned())));
}
