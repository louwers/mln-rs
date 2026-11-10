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
    // test/renderer/feature_tracking.test.cpp:152
}

// gtest: FeatureTracking.NDCBoundSymbol
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_symbol() {
    // test/renderer/feature_tracking.test.cpp:173
}

// gtest: FeatureTracking.NDCBoundSymbolOffset
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_symbol_offset() {
    // test/renderer/feature_tracking.test.cpp:206
}

// gtest: FeatureTracking.NDCBoundCircle
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle() {
    // test/renderer/feature_tracking.test.cpp:223
}

// gtest: FeatureTracking.NDCBoundCirclePitch1
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch1() {
    // test/renderer/feature_tracking.test.cpp:245
}

// gtest: FeatureTracking.NDCBoundCirclePitch2
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch2() {
    // test/renderer/feature_tracking.test.cpp:262
}

// gtest: FeatureTracking.NDCBoundCirclePitch3
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch3() {
    // test/renderer/feature_tracking.test.cpp:271
}

// gtest: FeatureTracking.NDCBoundCirclePitch4
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_circle_pitch4() {
    // test/renderer/feature_tracking.test.cpp:280
}

// gtest: FeatureTracking.NDCBoundFill
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_fill() {
    // test/renderer/feature_tracking.test.cpp:289
}

// gtest: FeatureTracking.NDCBoundTilt
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn ndc_bound_tilt() {
    // test/renderer/feature_tracking.test.cpp:308
}

// gtest: FeatureTracking.Overlap
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn overlap() {
    // test/renderer/feature_tracking.test.cpp:333
}

// gtest: FeatureTracking.ZeroOpacityConstant
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_constant() {
    // test/renderer/feature_tracking.test.cpp:345
}

// gtest: FeatureTracking.ZeroOpacityExpression
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_expression() {
    // test/renderer/feature_tracking.test.cpp:368
}

// gtest: FeatureTracking.ZeroOpacityDataDriven
#[test]
#[ignore = "TODO(port): rendered-feature tracking (layer feature capture with MapOptions::withRenderedFeatureInfo, RenderOrchestrator frameRenderedFeatures, Map::getRenderedFeatures/getRenderedFeatureCount, FeatureInfo::ndcBound)"]
fn zero_opacity_data_driven() {
    // test/renderer/feature_tracking.test.cpp:383
}
