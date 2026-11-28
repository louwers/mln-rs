// Projections that a host registers when it creates a runtime: the styles of
// the runtime's maps draw in them through the host's converter, the runtime
// refuses one it can't register, and the converter's user data is released
// once, after the runtime and what it made are gone.

#include <math.h>
#include <string.h>

#include "support/test_support.h"

// Counted from library threads and read from the test thread, so atomic.
typedef struct plane_probe {
  atomic_int calls;
  atomic_int releases;
} plane_probe;

// Converts as the built-in identity projection does: a coordinate's longitude
// and latitude are its point.
static void plane_forward(
  void* user_data, const mln_lat_lng* coordinates, size_t coordinate_count,
  mln_projected_point* out_points
) {
  atomic_fetch_add(&((plane_probe*)user_data)->calls, 1);
  for (size_t index = 0; index < coordinate_count; index += 1) {
    out_points[index] = (mln_projected_point){
      .x = coordinates[index].longitude, .y = coordinates[index].latitude
    };
  }
}

static void plane_inverse(
  void* user_data, const mln_projected_point* points, size_t point_count,
  mln_lat_lng* out_coordinates
) {
  atomic_fetch_add(&((plane_probe*)user_data)->calls, 1);
  for (size_t index = 0; index < point_count; index += 1) {
    out_coordinates[index] = (mln_lat_lng){
      .latitude = points[index].y, .longitude = points[index].x
    };
  }
}

static void plane_release(void* user_data) {
  atomic_fetch_add(&((plane_probe*)user_data)->releases, 1);
  mln_test_pulse();
}

// The identity's grid: tile 0/0/0 spans -90 to 90 on both axes.
static const mln_tile_matrix_set plane_grid = {
  .origin = {.x = -90.0, .y = 90.0}, .extent_at_zoom0 = 180.0
};

static mln_registered_projection plane(const char* name, plane_probe* probe) {
  return (mln_registered_projection){
    .size = sizeof(mln_registered_projection),
    .name = mln_buffer_view_from_cstr(name),
    .tile_matrix_set = plane_grid,
    .converter = {
      .forward = plane_forward,
      .inverse = plane_inverse,
      .user_data = probe,
      .release_user_data = plane_release,
    },
  };
}

static mln_screen_point pixel_at(
  mln_map_projection projection, mln_lat_lng coordinate
) {
  mln_screen_point point = {0};
  MLN_TEST_OK(
    mln_map_projection_pixel_for_lat_lng(projection, coordinate, &point, NULL)
  );
  return point;
}

// GL JS: a style draws in the projection that addProjection registered under
// its `projection.type`.
static void a_map_draws_in_a_projection_that_its_runtime_registers(void) {
  plane_probe probe = {0};
  mln_registered_projection projections[] = {plane("test:plane", &probe)};
  mln_runtime_options options = {.size = sizeof(mln_runtime_options)};
  options.projections = projections;
  options.projection_count = 1;
  mln_runtime runtime = mln_test_create_runtime_with_options(options);
  mln_map plane_map = mln_test_create_map(runtime);
  mln_map identity_map = mln_test_create_map(runtime);
  mln_test_load_style_and_wait(
    plane_map, MLN_BUFFER_LITERAL(
                 "{\"version\":8,\"projection\":{\"type\":\"test:plane\"},"
                 "\"center\":[10,20],\"zoom\":2,\"sources\":{},\"layers\":[]}"
               )
  );
  mln_test_load_style_and_wait(
    identity_map, MLN_BUFFER_LITERAL(
                    "{\"version\":8,\"projection\":{\"type\":\"identity\"},"
                    "\"center\":[10,20],\"zoom\":2,\"sources\":{},"
                    "\"layers\":[]}"
                  )
  );
  const mln_map_projection plane_projection =
    mln_test_map_create_projection(plane_map);
  const mln_map_projection identity_projection =
    mln_test_map_create_projection(identity_map);

  // The host's plane converts as the built-in identity does, through the
  // host's converter.
  const mln_lat_lng coordinate = {.latitude = 30.0, .longitude = 40.0};
  const int calls = atomic_load(&probe.calls);
  const mln_screen_point point = pixel_at(plane_projection, coordinate);
  TEST_ASSERT_GREATER_THAN_INT(calls, atomic_load(&probe.calls));
  const mln_screen_point expected = pixel_at(identity_projection, coordinate);
  TEST_ASSERT_DOUBLE_WITHIN(1e-9, expected.x, point.x);
  TEST_ASSERT_DOUBLE_WITHIN(1e-9, expected.y, point.y);
  mln_lat_lng back = {0};
  MLN_TEST_OK(
    mln_map_projection_lat_lng_for_pixel(plane_projection, point, &back, NULL)
  );
  TEST_ASSERT_DOUBLE_WITHIN(1e-9, 30.0, back.latitude);
  TEST_ASSERT_DOUBLE_WITHIN(1e-9, 40.0, back.longitude);

  // The user data stays the library's until the runtime, its maps and their
  // projections are released.
  mln_map_projection_release(plane_projection);
  mln_map_projection_release(identity_projection);
  mln_test_destroy_map(plane_map);
  mln_test_destroy_map(identity_map);
  TEST_ASSERT_EQUAL_INT(0, atomic_load(&probe.releases));
  mln_test_destroy_runtime(runtime);
  TEST_ASSERT_TRUE(mln_test_wait_for_count(&probe.releases, 1));
  TEST_ASSERT_EQUAL_INT(1, atomic_load(&probe.releases));
}

static void expect_refused(
  const mln_registered_projection* projections, size_t count,
  const char* fragment
) {
  mln_runtime_options options = {.size = sizeof(mln_runtime_options)};
  options.projections = projections;
  options.projection_count = count;
  mln_runtime runtime = MLN_HANDLE_NULL;
  MLN_TEST_INVALID(mln_runtime_create(&options, &runtime, MLN_TEST_DIAGNOSTIC));
  TEST_ASSERT_EQUAL_UINT64(MLN_HANDLE_NULL, runtime);
  TEST_ASSERT_NOT_NULL_MESSAGE(
    strstr(mln_test_last_error(), fragment), mln_test_last_error()
  );
}

// GL JS: addProjection refuses a name without a colon or one that is taken,
// and a zoom 0 extent that isn't positive. A refused creation leaves every
// converter's user data with the caller.
static void runtime_creation_refuses_a_projection_it_cant_register(void) {
  plane_probe probe = {0};
  mln_registered_projection projections[2];
  const mln_registered_projection valid[2] = {
    plane("test:plane", &probe), plane("test:other", &probe)
  };

  memcpy(projections, valid, sizeof(valid));
  projections[1].name = mln_buffer_view_from_cstr("plane");
  expect_refused(projections, 2, "needs a colon");

  memcpy(projections, valid, sizeof(valid));
  projections[1].name = mln_buffer_view_from_cstr("test:plane");
  expect_refused(projections, 2, "already registered");

  memcpy(projections, valid, sizeof(valid));
  projections[1].tile_matrix_set.origin.x = NAN;
  expect_refused(projections, 2, "finite origin");

  memcpy(projections, valid, sizeof(valid));
  projections[1].tile_matrix_set.extent_at_zoom0 = 0.0;
  expect_refused(projections, 2, "positive, finite extent");

  memcpy(projections, valid, sizeof(valid));
  projections[1].tile_matrix_set.extent_at_zoom0 = INFINITY;
  expect_refused(projections, 2, "positive, finite extent");

  memcpy(projections, valid, sizeof(valid));
  projections[1].converter.forward = NULL;
  expect_refused(projections, 2, "both forward and inverse");

  memcpy(projections, valid, sizeof(valid));
  projections[1].size = sizeof(mln_registered_projection) - 8;
  expect_refused(projections, 2, "projection 1 has an invalid size");

  expect_refused(NULL, 1, "projections must not be null");

  TEST_ASSERT_EQUAL_INT(0, atomic_load(&probe.calls));
  TEST_ASSERT_EQUAL_INT(0, atomic_load(&probe.releases));
}

MLN_TEST_GROUP {
  RUN_TEST(a_map_draws_in_a_projection_that_its_runtime_registers);
  RUN_TEST(runtime_creation_refuses_a_projection_it_cant_register);
}
