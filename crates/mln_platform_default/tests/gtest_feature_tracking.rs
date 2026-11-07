// Ports test/renderer/feature_tracking.test.cpp.
//
// Not ported yet: rendered-feature tracking. MapOptions::withRenderedFeatureInfo
// exists (crates/mln_map/src/map_options.rs) and reaches PaintParameters
// (`capture_rendered_features`), but nothing captures features: the layers'
// per-frame feature stats, RenderOrchestrator's frameRenderedFeatures
// (render_orchestrator.cpp:1032), RenderingStats::frameRenderedFeatures, and
// Map::getRenderedFeatures / getRenderedFeatureCount with
// FeatureInfo::ndcBound (map.cpp:620-674) are missing. Every test asserts on
// those, so the bodies stay comments pointing at the C++ test. What the
// fixture needs otherwise exists: Map::with_file_source (MapAdapter),
// StubFileSource, HeadlessFrontend, Map::set_debug, and the image check
// against test/fixtures/feature_tracking/<test>/expected.png.

// gtest: FeatureTracking.ApiQuery
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn api_query() {
    // test/renderer/feature_tracking.test.cpp:154
}

// gtest: FeatureTracking.NDCBoundSymbol
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_symbol() {
    // test/renderer/feature_tracking.test.cpp:175
}

// gtest: FeatureTracking.NDCBoundSymbolOffset
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_symbol_offset() {
    // test/renderer/feature_tracking.test.cpp:208
}

// gtest: FeatureTracking.NDCBoundTranslatedVariableAnchor
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_translated_variable_anchor() {
    // test/renderer/feature_tracking.test.cpp:226
}

// gtest: FeatureTracking.NDCBoundSharedBucketAtAnotherTranslation
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_shared_bucket_at_another_translation() {
    // test/renderer/feature_tracking.test.cpp:251
}

// gtest: FeatureTracking.NDCBoundCircle
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle() {
    // test/renderer/feature_tracking.test.cpp:274
}

// gtest: FeatureTracking.NDCBoundCirclePitch1
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch1() {
    // test/renderer/feature_tracking.test.cpp:296
}

// gtest: FeatureTracking.NDCBoundCirclePitch2
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch2() {
    // test/renderer/feature_tracking.test.cpp:313
}

// gtest: FeatureTracking.NDCBoundCirclePitch3
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch3() {
    // test/renderer/feature_tracking.test.cpp:322
}

// gtest: FeatureTracking.NDCBoundCirclePitch4
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch4() {
    // test/renderer/feature_tracking.test.cpp:331
}

// gtest: FeatureTracking.NDCBoundFill
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_fill() {
    // test/renderer/feature_tracking.test.cpp:340
}

// gtest: FeatureTracking.NDCBoundTilt
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_tilt() {
    // test/renderer/feature_tracking.test.cpp:359
}

// gtest: FeatureTracking.Overlap
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn overlap() {
    // test/renderer/feature_tracking.test.cpp:384
}

// gtest: FeatureTracking.ZeroOpacityConstant
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_constant() {
    // test/renderer/feature_tracking.test.cpp:396
}

// gtest: FeatureTracking.ZeroOpacityExpression
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_expression() {
    // test/renderer/feature_tracking.test.cpp:419
}

// gtest: FeatureTracking.ZeroOpacityDataDriven
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_data_driven() {
    // test/renderer/feature_tracking.test.cpp:434
}

// gtest: FeatureTracking.GlobeNDCBound
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_ndc_bound() {
    // test/renderer/feature_tracking.test.cpp:545
}

// gtest: FeatureTracking.GlobeNDCBoundCirclePitchAlignment
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_ndc_bound_circle_pitch_alignment() {
    // test/renderer/feature_tracking.test.cpp:560
}

// gtest: FeatureTracking.GlobeNDCBoundFillExtrusion
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_ndc_bound_fill_extrusion() {
    // test/renderer/feature_tracking.test.cpp:573
}

// gtest: FeatureTracking.GlobeFillExtrusionAboveHorizon
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_fill_extrusion_above_horizon() {
    // test/renderer/feature_tracking.test.cpp:592
}

// gtest: FeatureTracking.GlobeCaptureDuringTransition
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_capture_during_transition() {
    // test/renderer/feature_tracking.test.cpp:603
}

// gtest: FeatureTracking.GlobeCaptureCloseUp
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_capture_close_up() {
    // test/renderer/feature_tracking.test.cpp:617
}

// gtest: FeatureTracking.GlobeOccluded
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn globe_occluded() {
    // test/renderer/feature_tracking.test.cpp:633
}
