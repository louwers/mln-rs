// Ports test/api/location_indicator.test.cpp. The port registers the
// location-indicator layer type in the style but has no
// RenderLocationIndicatorLayer yet, so each test is an ignored stub that
// says what it checks.

// gtest: LocationIndicator.MercatorPuckAtItsLocation
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:254"]
fn mercator_puck_at_its_location() {
    // test/api/location_indicator.test.cpp:254: on Mercator, the accuracy
    // circle of a puck at 20, 20 is drawn within 3 pixels of pixelForLatLng.
}

// gtest: LocationIndicator.PuckAtItsLocationWithTheCenterPastTheAntimeridian
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:259"]
fn puck_at_its_location_with_the_center_past_the_antimeridian() {
    // test/api/location_indicator.test.cpp:259: bounds keep the center
    // unwrapped past the antimeridian, at 181 degrees; the puck is drawn, and
    // hit, where it is.
}

// gtest: LocationIndicator.GlobePuckAtItsLocation
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:311"]
fn globe_puck_at_its_location() {
    // test/api/location_indicator.test.cpp:311: the Mercator test's accuracy
    // circle, on the globe.
}

// gtest: LocationIndicator.GlobePuckKeepsItsSize
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:320"]
fn globe_puck_keeps_its_size() {
    // test/api/location_indicator.test.cpp:320: the puck lies flat on the
    // sphere: the surface turning away narrows it, nothing makes it larger
    // than the 16 pixels its image has at the map's center, and Mercator
    // keeps that size at every latitude.
}

// gtest: LocationIndicator.GlobePuckCapturedWhereItIsDrawn
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:355"]
fn globe_puck_captured_where_it_is_drawn() {
    // test/api/location_indicator.test.cpp:355: the capture reports the puck
    // where the globe draws it, and nothing for a puck behind the horizon.
}

// gtest: LocationIndicator.GlobePuckCapturedWhereItIsDrawnMidTransition
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:383"]
fn globe_puck_captured_where_it_is_drawn_mid_transition() {
    // test/api/location_indicator.test.cpp:383: half way between Mercator and
    // the globe, the capture follows the blend the puck is drawn with.
}

// gtest: LocationIndicator.PuckDrawnWhereItIsWithTheCenterPastTheAntimeridian
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:407"]
fn puck_drawn_where_it_is_with_the_center_past_the_antimeridian() {
    // test/api/location_indicator.test.cpp:407: with the globe's center on
    // the next world copy, at 181 degrees, the puck's Mercator side is placed
    // in that copy too.
}

// gtest: LocationIndicator.PuckDrawnAfterTheProjectionChanges
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:433"]
fn puck_drawn_after_the_projection_changes() {
    // test/api/location_indicator.test.cpp:433: changing the projection
    // rebuilds the puck's drawables, which take its corners and image again
    // without the camera or the puck changing.
}

// gtest: LocationIndicator.GlobeHatAndShadowLeanUpAndDownTheScreenWithTheWindowBottomInSpace
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:467"]
fn globe_hat_and_shadow_lean_up_and_down_the_screen_with_the_window_bottom_in_space() {
    // test/api/location_indicator.test.cpp:467: pitched 20 degrees at zoom 0,
    // where the bottom of the window is past the globe's edge, the hat and
    // the shadow of a puck at 0, 40 lean straight up and down the screen.
}

// gtest: LocationIndicator.GlobeHatAndShadowLeanUpAndDownTheScreenBesideTheCenter
#[test]
#[ignore = "TODO(port): RenderLocationIndicatorLayer (the location-indicator layer type is registered in the style but not rendered); see location_indicator.test.cpp:474"]
fn globe_hat_and_shadow_lean_up_and_down_the_screen_beside_the_center() {
    // test/api/location_indicator.test.cpp:474: the same pitched 30 degrees
    // at zoom 2, for a puck at 20, 20 beside the center of the window.
}
