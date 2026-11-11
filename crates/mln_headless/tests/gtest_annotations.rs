// Ports test/api/annotations.test.cpp (a module of the gtest_api test
// crate). The annotation API (`Map::addAnnotation`, `addAnnotationImage`,
// `updateAnnotation`, `removeAnnotation`, `getTopOffsetPixelsForAnnotationImage`)
// and the AnnotationManager/annotation source behind it are not ported, so
// every test that adds an annotation is ignored; each body names what the C++ test checks (the
// fixtures are test/fixtures/annotations/<name>/expected.png, compared with
// checkImage(.., 0.0002, 0.1)).

use std::sync::Arc;

use mln_map::{CameraOptions, MapDebugOptions, MapMode};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};

use crate::support::*;

/// `AnnotationTest`: a static 256x256 map over a `StubFileSource()`.
// TODO(port): `StubFileSource()` uses the MapTiler tile server options.
struct AnnotationTest(MapTest);

impl AnnotationTest {
    fn new() -> Self {
        let run_loop = mln_actor::RunLoop::new();
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        AnnotationTest(MapTest::create(run_loop, file_source, 1.0, MapMode::Static))
    }

    fn map(&self) -> crate::support::MapGuard<'_> {
        self.0.map()
    }

    fn check_rendering(&self, name: &str) {
        check_image(&format!("test/fixtures/annotations/{name}"), &self.0.render().image, 0.0002, 0.1);
    }
}

// gtest: Annotations.SymbolAnnotation
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:51"]
fn symbol_annotation() {
    // a symbol annotation with an added annotation image renders (point_annotation).
}

// gtest: Annotations.SymbolAnnotationTileBoundary
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:69"]
fn symbol_annotation_tile_boundary() {
    // a symbol annotation a fraction of a degree off 0,0 at z10 renders (point_annotation; issue 12472).
}

// gtest: Annotations.LineAnnotation
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:82"]
fn line_annotation() {
    // a red 5 px line annotation renders, also at the max zoom (line_annotation, line_annotation_max_zoom).
}

// gtest: Annotations.FillAnnotation
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:98"]
fn fill_annotation() {
    // a red fill annotation renders, also at the max zoom (fill_annotation, fill_annotation_max_zoom).
}

// gtest: Annotations.AntimeridianAnnotationSmall
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:113"]
fn antimeridian_annotation_small() {
    // a line and a small fill annotation across the antimeridian at z0 (antimeridian_annotation_small).
}

// gtest: Annotations.AntimeridianAnnotationLarge
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:134"]
fn antimeridian_annotation_large() {
    // a line and a fill annotation spanning the world the long way round (antimeridian_annotation_large).
}

// gtest: Annotations.OverlappingFillAnnotation
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:155"]
fn overlapping_fill_annotation() {
    // two overlapping fill annotations (overlapping_fill_annotation).
}

// gtest: Annotations.AddMultiple
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:170"]
fn add_multiple() {
    // an annotation added after a render shows next to the first (add_multiple).
}

// gtest: Annotations.NonImmediateAdd
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:183"]
fn non_immediate_add() {
    // a fill annotation added after the first render shows (non_immediate_add).
}

// gtest: Annotations.UpdateSymbolAnnotationGeometry
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:197"]
fn update_symbol_annotation_geometry() {
    // updateAnnotation moves a symbol (update_point).
}

// gtest: Annotations.UpdateSymbolAnnotationIcon
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:211"]
fn update_symbol_annotation_icon() {
    // updateAnnotation changes a symbol's icon (update_icon).
}

// gtest: Annotations.UpdateLineAnnotationGeometry
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:225"]
fn update_line_annotation_geometry() {
    // updateAnnotation changes a line's geometry (update_line_geometry).
}

// gtest: Annotations.UpdateLineAnnotationStyle
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:242"]
fn update_line_annotation_style() {
    // updateAnnotation changes a line's color and width (update_line_style).
}

// gtest: Annotations.UpdateFillAnnotationGeometry
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:260"]
fn update_fill_annotation_geometry() {
    // updateAnnotation changes a fill's geometry (update_fill_geometry).
}

// gtest: Annotations.UpdateFillAnnotationStyle
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:276"]
fn update_fill_annotation_style() {
    // updateAnnotation changes a fill's color (update_fill_style).
}

// gtest: Annotations.RemovePoint
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:293"]
fn remove_point() {
    // removeAnnotation removes a symbol (remove_point).
}

// gtest: Annotations.RemoveShape
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:306"]
fn remove_shape() {
    // removeAnnotation removes a line (remove_shape).
}

// gtest: Annotations.ImmediateRemoveShape
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:323"]
fn immediate_remove_shape() {
    // removing a shape annotation right after adding it does not crash.
}

// gtest: Annotations.SwitchStyle
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:332"]
fn switch_style() {
    // a symbol annotation survives loading the style again (switch_style).
}

// gtest: Annotations.ReaddImage
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:345"]
fn readd_image() {
    // re-adding an annotation image under the same name updates the symbol (readd_image).
}

// gtest: Annotations.QueryRenderedFeatures
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:358"]
fn query_rendered_features() {
    // queryRenderedFeatures finds the two symbol annotations by id at their pixels.
}

// gtest: Annotations.QueryFractionalZoomLevels
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:391"]
fn query_fractional_zoom_levels() {
    // symbol annotations stay queryable at fractional zoom levels.
}

// gtest: Annotations.VisibleFeatures
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:429"]
fn visible_features() {
    // a box query over the viewport returns each visible symbol annotation once, also after rotation.
}

// gtest: Annotations.ViewFrustumCulling
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:472"]
fn view_frustum_culling() {
    // with a pitched camera, the annotations inside the frustum are returned by a viewport query (latLngsForPixels batch conversion included).
}

// gtest: Annotations.TopOffsetPixels
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:560"]
fn top_offset_pixels() {
    // getTopOffsetPixelsForAnnotationImage(default_marker) is -28.
}

// gtest: Annotations.DebugEmpty
#[test]
fn debug_empty() {
    // This test should render nothing, not even the tile borders. Tile borders
    // are only rendered when there is an actual tile we're trying to render,
    // but since there is no annotation, we should not render them.
    let test = AnnotationTest::new();

    test.map().style_mut().load_json(&read_file("test/fixtures/api/empty.json"));
    test.map().set_debug(MapDebugOptions::TILE_BORDERS);
    test.map().jump_to(&CameraOptions::default().with_zoom(Some(1.0)));

    test.check_rendering("debug_empty");
}

// gtest: Annotations.DebugSparse
#[test]
#[ignore = "OpenGL-only in C++ (returns early on other backends); also TODO(port): annotations; see annotations.test.cpp:580"]
fn debug_sparse() {
    // With tile borders on at z1, only the top right tile (holding one
    // symbol annotation) renders with its border (debug_sparse).
}

// gtest: Annotations.ChangeMaxZoom
#[test]
#[ignore = "TODO(port): annotations (Map::addAnnotation*, AnnotationManager, the annotation source); see annotations.test.cpp:598"]
fn change_max_zoom() {
    // a line annotation renders after setBounds lowers the max zoom (line_annotation_max_zoom).
}
