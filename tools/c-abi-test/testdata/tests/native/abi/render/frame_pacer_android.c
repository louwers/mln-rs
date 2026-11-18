// Frame pacing on Android, which demands a session's frames at the vsyncs that
// the calling looper's AChoreographer reports.
//
// These executables have no app main thread, so each case that paces a
// session runs a thread that prepares an ALooper, sets the session's pacing
// there, and polls the looper as an app's main thread does. The thread stops
// before the case ends, even after a failed assertion: a thread that polls its
// looper while the process exits can crash in libbinder.

#include <android/looper.h>
#include <pthread.h>

#include "support/frames.h"
#include "support/test_support.h"

typedef struct looper_thread {
  pthread_t thread;
  ALooper* looper;
  mln_render_session session;
  const mln_frame_pacing_options* options;
  mln_status status;
  atomic_bool ready;
  atomic_bool stop;
} looper_thread;

static void* run_looper(void* context) {
  looper_thread* state = context;
  state->looper = ALooper_prepare(0);
  state->status =
    mln_render_session_set_frame_pacing(state->session, state->options, NULL);
  mln_test_flag_set(&state->ready);
  while (!atomic_load(&state->stop)) {
    (void)ALooper_pollOnce(-1, NULL, NULL, NULL);
  }
  return NULL;
}

static void* set_pacing_without_looper(void* context) {
  looper_thread* state = context;
  state->status =
    mln_render_session_set_frame_pacing(state->session, state->options, NULL);
  return NULL;
}

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

// Paces a new session with `options` from a looper thread, and expects each
// camera change to reach a frame that no one else demands.
static void expect_paced_frames(const mln_frame_pacing_options* options) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_test_render_prepare_map(map);
  mln_test_render_fixture fixture = {0};
  TEST_ASSERT_TRUE(mln_test_render_fixture_create_offscreen(map, &fixture));
  looper_thread state = {.session = fixture.session, .options = options};
  if (pthread_create(&state.thread, NULL, run_looper, &state) != 0) {
    TEST_FAIL_MESSAGE("could not start the looper thread");
  }
  if (TEST_PROTECT()) {
    TEST_ASSERT_TRUE(mln_test_wait_for_flag(&state.ready));
    MLN_TEST_OK(state.status);
    mln_test_render_wait_for_update(&fixture, move_camera(map, 2.0));
    mln_test_render_wait_for_update(&fixture, move_camera(map, 3.0));
  }
  // The session lets go of its pacer, which ends the looper's watches.
  mln_test_render_fixture_destroy(&fixture);
  if (mln_test_wait_for_flag(&state.ready)) {
    atomic_store(&state.stop, true);
    ALooper_wake(state.looper);
    pthread_join(state.thread, NULL);
  }
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

static void a_thread_without_a_looper_takes_no_frame_pacing(void) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_test_render_fixture fixture = {0};
  TEST_ASSERT_TRUE(mln_test_render_fixture_create_offscreen(map, &fixture));
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  looper_thread state = {.session = fixture.session, .options = &options};
  TEST_ASSERT_EQUAL_INT(
    0, pthread_create(&state.thread, NULL, set_pacing_without_looper, &state)
  );
  TEST_ASSERT_EQUAL_INT(0, pthread_join(state.thread, NULL));
  TEST_ASSERT_EQUAL_INT(MLN_STATUS_INVALID_STATE, state.status);
  mln_test_render_fixture_destroy(&fixture);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

static void a_paced_session_draws_each_change_without_a_demand(void) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  expect_paced_frames(&options);
}

static void a_session_paced_with_a_frame_rate_limit_draws_each_change(void) {
  mln_frame_pacing_options options = {.size = sizeof(mln_frame_pacing_options)};
  options.fields = MLN_FRAME_PACING_OPTION_MAXIMUM_FRAMES_PER_SECOND;
  options.maximum_frames_per_second = 30;
  expect_paced_frames(&options);
}

MLN_TEST_GROUP {
  RUN_TEST(a_thread_without_a_looper_takes_no_frame_pacing);
  RUN_TEST(a_paced_session_draws_each_change_without_a_demand);
  RUN_TEST(a_session_paced_with_a_frame_rate_limit_draws_each_change);
}
