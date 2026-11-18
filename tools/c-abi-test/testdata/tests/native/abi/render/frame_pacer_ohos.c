// Frame pacing on OpenHarmony, which demands a session's frames at the vsyncs
// that an OH_NativeVSync reports. The native VSync calls back on a thread of
// its own, so these cases need no event loop.

#include "support/frames.h"
#include "support/test_support.h"

// Returns the render update generation that shows the change.
static uint64_t move_camera(mln_map map, double zoom) {
  mln_camera_update update = {
    .size = sizeof(mln_camera_update),
    .camera = {.size = sizeof(mln_camera_options)}
  };
  update.camera.fields = MLN_CAMERA_OPTION_ZOOM;
  update.camera.zoom = zoom;
  mln_test_completion completion = mln_test_completion_default(0);
  MLN_TEST_OK(
    mln_map_update_camera(map, &update, &completion.descriptor, NULL)
  );
  MLN_TEST_OK(mln_test_completion_finish(&completion));
  const uint64_t generation =
    mln_test_completion_render_update_generation(&completion);
  mln_test_completion_destroy(&completion);
  return generation;
}

// Paces a new session with `options`, then with `changed`, and expects a
// camera change under each to reach a frame that no one else demands.
static void expect_paced_frames(
  const mln_frame_pacing_options* options,
  const mln_frame_pacing_options* changed
) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_test_render_prepare_map(map);
  mln_test_render_fixture fixture = {0};
  TEST_ASSERT_TRUE(mln_test_render_fixture_create_offscreen(map, &fixture));
  MLN_TEST_OK(mln_render_session_set_frame_pacing(
    fixture.session, options, MLN_TEST_DIAGNOSTIC
  ));
  mln_test_render_wait_for_update(&fixture, move_camera(map, 2.0));
  MLN_TEST_OK(mln_render_session_set_frame_pacing(
    fixture.session, changed, MLN_TEST_DIAGNOSTIC
  ));
  mln_test_render_wait_for_update(&fixture, move_camera(map, 3.0));

  mln_test_render_fixture_destroy(&fixture);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

static void a_paced_session_draws_each_change_without_a_demand(void) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  expect_paced_frames(&options, &options);
}

static void a_paced_session_follows_a_new_frame_rate_limit(void) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  mln_frame_pacing_options limited = options;
  limited.fields = MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND;
  limited.maximum_frames_per_second = 30;
  expect_paced_frames(&limited, &options);
}

MLN_TEST_GROUP {
  RUN_TEST(a_paced_session_draws_each_change_without_a_demand);
  RUN_TEST(a_paced_session_follows_a_new_frame_rate_limit);
}
