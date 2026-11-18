// mln_render_session_set_frame_pacing() on every platform: its options and its
// session. The files tagged _apple, _android, and _ohos test the pacing of the
// platforms with a display to follow.

#if defined(__APPLE__)
#include <TargetConditionals.h>
#endif

#include "support/frames.h"
#include "support/test_support.h"

#if !defined(__APPLE__) && !defined(__ANDROID__) && !defined(__OHOS__)
#define NO_DISPLAY_SOURCE 1
#endif

static void with_session(void (*body)(mln_render_session session)) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_test_render_fixture fixture = {0};
  TEST_ASSERT_TRUE(mln_test_render_fixture_create_offscreen(map, &fixture));
  body(fixture.session);
  mln_test_render_fixture_destroy(&fixture);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

#define SESSION_CASE(name)                               \
  static void name##_body(mln_render_session session);   \
  static void name(void) { with_session(name##_body); }  \
  static void name##_body(mln_render_session session)

// Options are checked before the library looks for a display.
SESSION_CASE(invalid_frame_pacing_options_are_refused) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  options.fields = MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND;
  options.maximum_frames_per_second = 0;
  MLN_TEST_INVALID(mln_render_session_set_frame_pacing(session, &options, NULL));

  options.fields = MLN_FRAME_PACING_OPTION_VIEW;
  options.view = NULL;
  MLN_TEST_INVALID(mln_render_session_set_frame_pacing(session, &options, NULL));
}

// Null options stop pacing, which a session that isn't paced takes too.
SESSION_CASE(null_options_stop_pacing) {
  MLN_TEST_OK(mln_render_session_set_frame_pacing(session, NULL, NULL));
}

#if !defined(__APPLE__) || !TARGET_OS_OSX
// Only macOS paces to a view's display.
SESSION_CASE(a_view_needs_macos) {
  int view = 0;
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  options.fields = MLN_FRAME_PACING_OPTION_VIEW;
  options.view = &view;
  MLN_TEST_STATUS(
    MLN_STATUS_UNSUPPORTED,
    mln_render_session_set_frame_pacing(session, &options, MLN_TEST_DIAGNOSTIC)
  );
}
#endif

#if defined(NO_DISPLAY_SOURCE)
SESSION_CASE(a_platform_without_a_display_source_has_no_frame_pacing) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  MLN_TEST_STATUS(
    MLN_STATUS_UNSUPPORTED,
    mln_render_session_set_frame_pacing(session, &options, MLN_TEST_DIAGNOSTIC)
  );
}
#endif

static void a_session_with_a_redraw_wake_takes_no_frame_pacing(void) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_render_session_attach_options options = {
    .size = sizeof(mln_render_session_attach_options)
  };
  options.redraw_wake = mln_test_pulse_wake();
  mln_test_completion attach = mln_test_completion_default(0);
  mln_test_render_fixture fixture = {0};
  MLN_TEST_OK(mln_test_render_fixture_start_attach(
    map, &options, &attach.descriptor, &fixture
  ));
  MLN_TEST_OK(mln_test_render_fixture_finish_operation(&fixture, &attach));
  mln_test_completion_destroy(&attach);

  MLN_TEST_INVALID_STATE(
    mln_render_session_set_frame_pacing(fixture.session, NULL, NULL)
  );

  mln_test_render_fixture_destroy(&fixture);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

MLN_TEST_GROUP {
  RUN_TEST(invalid_frame_pacing_options_are_refused);
  RUN_TEST(null_options_stop_pacing);
#if !defined(__APPLE__) || !TARGET_OS_OSX
  RUN_TEST(a_view_needs_macos);
#endif
#if defined(NO_DISPLAY_SOURCE)
  RUN_TEST(a_platform_without_a_display_source_has_no_frame_pacing);
#endif
  RUN_TEST(a_session_with_a_redraw_wake_takes_no_frame_pacing);
}
