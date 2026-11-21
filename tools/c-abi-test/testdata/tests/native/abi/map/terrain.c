// A map over terrain: a projection of the map meets the ground that it drew
// and reads its heights.

#include <math.h>

#include "support/map.h"
#include "support/resources.h"
#include "support/test_support.h"

// A 4x4 DEM tile of ground 1000 m high everywhere, in the Mapbox encoding:
// (1 * 65536 + 173 * 256 + 176) * 0.1 - 10000.
static const uint8_t level_dem_png[] = {
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
  0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04,
  0x08, 0x06, 0x00, 0x00, 0x00, 0xa9, 0xf1, 0x9e, 0x7e, 0x00, 0x00, 0x00,
  0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x60, 0x5c, 0xbb, 0xe1,
  0x3f, 0x32, 0x66, 0x20, 0x5d, 0x00, 0x00, 0xee, 0x38, 0x25, 0xd1, 0x5b,
  0xe2, 0x5b, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
  0x42, 0x60, 0x82,
};

static const mln_test_provided_resource level_dem[] = {
  {.url = "test://dem/0/0/0.png",
   .response =
     {
       .status = MLN_RESOURCE_RESPONSE_STATUS_OK,
       .bytes = level_dem_png,
       .byte_count = sizeof(level_dem_png),
     }},
};

static const char level_terrain_style[] =
  "{\"version\":8,\"center\":[20,10],\"zoom\":11,\"pitch\":60,"
  "\"terrain\":{\"source\":\"dem\"},"
  "\"sources\":{\"dem\":{\"type\":\"raster-dem\","
  "\"tiles\":[\"test://dem/{z}/{x}/{y}.png\"],\"maxzoom\":0}},"
  "\"layers\":[{\"id\":\"background\",\"type\":\"background\"}]}";

static mln_map_projection_terrain_elevation_result elevation_at(
  mln_map_projection projection, double latitude, double longitude
) {
  mln_map_projection_terrain_elevation_result result = {
    .size = sizeof(mln_map_projection_terrain_elevation_result)
  };
  MLN_TEST_OK(mln_map_projection_get_terrain_elevation(
    projection, (mln_lat_lng){.latitude = latitude, .longitude = longitude},
    &result, NULL
  ));
  return result;
}

static mln_map_projection create_projection(mln_map map) {
  mln_map_projection projection = MLN_HANDLE_NULL;
  MLN_TEST_OK(mln_map_create_projection(map, &projection, NULL));
  return projection;
}

// The map's center stays at sea level, 1000 m under the ground and the camera
// 1800 m above it, so a point below the center meets the ground nearer the
// camera than the flat map's sea level. A map without skirts draws the same
// ground.
static void a_maps_projection_meets_and_reads_the_drawn_ground(void) {
  mln_test_provider* provider = mln_test_provider_create(level_dem, 1);
  mln_runtime runtime = mln_test_create_runtime();
  mln_test_provider_install(runtime, provider);
  mln_map_options options = mln_test_map_options();
  options.initial_extent =
    (mln_logical_extent){.width = 64, .height = 64, .scale_factor = 1.0};
  options.map_mode = MLN_MAP_MODE_STATIC;
  options.terrain_skirt_length = MLN_TERRAIN_SKIRT_LENGTH_NONE;
  mln_map map = mln_test_create_map_with_options(runtime, &options);
  MLN_TEST_AWAIT_OK(mln_map_set_center_clamped_to_ground_enabled(
    map, false, &completion.descriptor, NULL
  ));
  mln_test_load_style_and_wait(map, MLN_BUFFER_LITERAL(level_terrain_style));
  mln_test_render_fixture fixture = {0};
  TEST_ASSERT_TRUE(mln_test_render_fixture_create_offscreen(map, &fixture));

  // A static map draws nothing until a still image asks, so there's no
  // ground to read yet.
  mln_map_projection projection = create_projection(map);
  TEST_ASSERT_FALSE(elevation_at(projection, 10.0, 20.0).has_elevation);
  mln_map_projection_release(projection);

  MLN_TEST_OK(mln_test_render_still_image(&fixture, map));
  projection = create_projection(map);
  const mln_map_projection_terrain_elevation_result center =
    elevation_at(projection, 10.0, 20.0);
  TEST_ASSERT_TRUE(center.has_elevation);
  TEST_ASSERT_DOUBLE_WITHIN(0.1, 1000.0, center.elevation_meters);
  TEST_ASSERT_FALSE(elevation_at(projection, -60.0, -100.0).has_elevation);

  const mln_map_projection flat = mln_test_map_create_projection(map);
  TEST_ASSERT_FALSE(elevation_at(flat, 10.0, 20.0).has_elevation);
  const mln_screen_point below = {.x = 32.0, .y = 56.0};
  mln_lat_lng on_ground = {0};
  mln_lat_lng at_sea_level = {0};
  MLN_TEST_OK(
    mln_map_projection_lat_lng_for_pixel(projection, below, &on_ground, NULL)
  );
  MLN_TEST_OK(
    mln_map_projection_lat_lng_for_pixel(flat, below, &at_sea_level, NULL)
  );
  TEST_ASSERT_TRUE(at_sea_level.latitude - on_ground.latitude > 1e-3);
  mln_screen_point back = {0};
  MLN_TEST_OK(
    mln_map_projection_pixel_for_lat_lng(projection, on_ground, &back, NULL)
  );
  TEST_ASSERT_DOUBLE_WITHIN(0.01, below.x, back.x);
  TEST_ASSERT_DOUBLE_WITHIN(0.01, below.y, back.y);

  mln_map_projection_release(flat);
  mln_map_projection_release(projection);
  mln_test_render_fixture_destroy(&fixture);
  mln_test_destroy_map(map);
  mln_test_destroy_runtime(runtime);
  mln_test_provider_destroy(provider);
}

MLN_TEST_GROUP { RUN_TEST(a_maps_projection_meets_and_reads_the_drawn_ground); }
