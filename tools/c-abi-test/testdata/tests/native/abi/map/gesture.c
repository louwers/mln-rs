// A gesture recognizer's calls and the structs they pass: taps come back as
// events of the input that made them, a box-zoom drag reports its box, and
// options out of range are rejected.

#include "support/test_support.h"

static mln_gesture_input pointer_down(uint32_t id, double x, double y) {
  mln_gesture_input input = {
    .size = sizeof(mln_gesture_input), .type = MLN_GESTURE_INPUT_POINTER_DOWN
  };
  input.data.pointer_down = (mln_gesture_pointer_down){
    .id = id,
    .kind = MLN_POINTER_KIND_MOUSE,
    .position = {.x = x, .y = y},
  };
  return input;
}

static mln_gesture_input pointer(
  mln_gesture_input_type type, uint32_t id, double x, double y
) {
  mln_gesture_input input = {.size = sizeof(mln_gesture_input), .type = type};
  const mln_gesture_pointer moved = {.id = id, .position = {.x = x, .y = y}};
  if (type == MLN_GESTURE_INPUT_POINTER_MOVE) {
    input.data.pointer_move = moved;
  } else {
    input.data.pointer_up = moved;
  }
  return input;
}

static mln_gesture_result handle(
  mln_gesture_recognizer recognizer, mln_gesture_input input, double time_ms
) {
  mln_gesture_result result = {.size = sizeof(mln_gesture_result)};
  MLN_TEST_OK(
    mln_gesture_recognizer_handle(recognizer, &input, time_ms, &result, NULL)
  );
  return result;
}

static void taps_and_box_zooms_come_back_in_results(void) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_gesture_recognizer recognizer = MLN_HANDLE_NULL;
  MLN_TEST_OK(mln_gesture_recognizer_create(map, NULL, &recognizer, NULL));

  mln_gesture_result result = handle(recognizer, pointer_down(1, 20, 20), 0);
  TEST_ASSERT_TRUE(result.handled);
  result = handle(
    recognizer, pointer(MLN_GESTURE_INPUT_POINTER_UP, 1, 20, 20), 50
  );
  TEST_ASSERT_EQUAL_size_t(1, result.event_count);
  TEST_ASSERT_EQUAL_UINT32(MLN_GESTURE_EVENT_TAP, result.events[0].type);
  TEST_ASSERT_EQUAL_UINT32(1, result.events[0].data.tap.id);
  TEST_ASSERT_EQUAL_DOUBLE(20.0, result.events[0].data.tap.position.x);

  handle(recognizer, pointer_down(1, 20, 20), 100);
  result = handle(
    recognizer, pointer(MLN_GESTURE_INPUT_POINTER_UP, 1, 20, 20), 150
  );
  TEST_ASSERT_EQUAL_size_t(2, result.event_count);
  TEST_ASSERT_EQUAL_UINT32(MLN_GESTURE_EVENT_DOUBLE_TAP, result.events[1].type);

  mln_gesture_input box = pointer_down(2, 10, 10);
  box.data.pointer_down.modes = MLN_GESTURE_MODE_BOX_ZOOM;
  handle(recognizer, box, 1000);
  result = handle(
    recognizer, pointer(MLN_GESTURE_INPUT_POINTER_MOVE, 2, 40, 30), 1016
  );
  TEST_ASSERT_TRUE(result.box_zoom_active);
  TEST_ASSERT_EQUAL_DOUBLE(10.0, result.box_zoom.min.x);
  TEST_ASSERT_EQUAL_DOUBLE(40.0, result.box_zoom.max.x);
  mln_gesture_input cancel = {
    .size = sizeof(mln_gesture_input), .type = MLN_GESTURE_INPUT_POINTER_CANCEL
  };
  cancel.data.pointer_cancel.id = 2;
  result = handle(recognizer, cancel, 1032);
  TEST_ASSERT_FALSE(result.box_zoom_active);

  mln_gesture_recognizer_release(recognizer);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

static void options_out_of_range_are_rejected(void) {
  mln_runtime runtime = mln_test_create_runtime();
  mln_map map = mln_test_create_map(runtime);
  mln_gesture_options options = {.size = sizeof(mln_gesture_options)};
  const mln_gesture_notch notch = {.spacing = 0.0};
  options.bearing.fields = MLN_GESTURE_AXIS_OPTION_NOTCHES;
  options.bearing.notches = &notch;
  options.bearing.notch_count = 1;
  mln_gesture_recognizer recognizer = MLN_HANDLE_NULL;
  MLN_TEST_INVALID(
    mln_gesture_recognizer_create(map, &options, &recognizer, NULL)
  );

  MLN_TEST_OK(mln_gesture_recognizer_create(map, NULL, &recognizer, NULL));
  options = (mln_gesture_options){.size = sizeof(mln_gesture_options)};
  options.pan.fields = MLN_GESTURE_PAN_OPTION_MOMENTUM;
  options.pan.momentum.fields =
    MLN_MOMENTUM_DECAY_MODEL | MLN_MOMENTUM_DECAY_HALF_LIFE_MS;
  options.pan.momentum.model = MLN_MOMENTUM_DECAY_MODEL_CURVE;
  options.pan.momentum.half_life_ms = 100.0;
  MLN_TEST_INVALID(
    mln_gesture_recognizer_set_options(recognizer, &options, NULL)
  );
  options.pan.momentum.fields = MLN_MOMENTUM_DECAY_MODEL;
  MLN_TEST_OK(mln_gesture_recognizer_set_options(recognizer, &options, NULL));

  mln_gesture_recognizer_release(recognizer);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
}

MLN_TEST_GROUP {
  RUN_TEST(taps_and_box_zooms_come_back_in_results);
  RUN_TEST(options_out_of_range_are_rejected);
}
