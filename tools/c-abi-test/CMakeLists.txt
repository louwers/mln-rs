# Builds maplibre-native-ffi's ABI suite (testdata/tests/native/abi) against
# crates/mln_c's headers and library, the way maplibre-native-ffi's
# cmake/mln_ffi_tests.cmake builds it against its own: the same support files,
# Unity and graphics fixtures, for one render backend: vulkan, or opengl on EGL,
# maplibre-native-ffi's Linux OpenGL preset. `just c-abi-test` configures it
# with the library that Cargo built and runs it through run.py.
cmake_minimum_required(VERSION 3.25)
project(mln_c_abi_test C CXX)

set(MLN_C_LIBRARY "" CACHE FILEPATH "crates/mln_c's shared library")
# The backend the suite drives its fixtures with: Vulkan, which lavapipe
# provides without a GPU, or OpenGL on EGL, which llvmpipe does.
set(MLN_C_TEST_BACKEND vulkan CACHE STRING "The render backend to test")
set_property(CACHE MLN_C_TEST_BACKEND PROPERTY STRINGS vulkan opengl)
if(NOT EXISTS "${MLN_C_LIBRARY}")
  message(FATAL_ERROR "MLN_C_LIBRARY must name crates/mln_c's shared library")
endif()
if(NOT MLN_C_TEST_BACKEND MATCHES "^(vulkan|opengl)$")
  message(FATAL_ERROR "MLN_C_TEST_BACKEND must be vulkan or opengl")
endif()

set(MLN_C_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../crates/mln_c/include")
set(TESTS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/testdata/tests/native")
set(GRAPHICS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/testdata/tests/graphics")
set(SUPPORT_DIR "${TESTS_DIR}/support")

find_package(Threads REQUIRED)
find_path(VULKAN_INCLUDE_DIR vulkan/vulkan_core.h REQUIRED)
find_library(VULKAN_LOADER_LIBRARY NAMES vulkan vulkan.1 REQUIRED)
# The OpenGL cases make EGL contexts and GL textures of their own.
if(MLN_C_TEST_BACKEND STREQUAL "opengl")
  find_library(EGL_LIBRARY NAMES EGL REQUIRED)
  find_library(GLES_LIBRARY NAMES GLESv2 REQUIRED)
endif()

# The library, under the name of maplibre-native-ffi's target. Cargo gives it
# no soname, so the suite links it by directory and name.
add_library(maplibre_native_c SHARED IMPORTED)
set_target_properties(
  maplibre_native_c
  PROPERTIES IMPORTED_LOCATION "${MLN_C_LIBRARY}"
             IMPORTED_NO_SONAME TRUE
             INTERFACE_INCLUDE_DIRECTORIES "${MLN_C_INCLUDE_DIR}")

# The suite's domains and file tags, as maplibre-native-ffi lists them. A file
# whose name ends in a tag builds only where the tag is active.
set(ABI_DOMAINS
    base
    completion
    runtime
    resources
    map
    projection
    style
    render
    backend
    delivery
    binding
    platform)
set(FILE_TAGS
    metal
    opengl
    vulkan
    webgpu
    egl
    wgl
    webgl
    emscripten
    android
    ohos
    apple
    wayland
    linux
    windows
    icu)
set(ACTIVE_TAGS ${MLN_C_TEST_BACKEND})
if(MLN_C_TEST_BACKEND STREQUAL "opengl")
  list(APPEND ACTIVE_TAGS egl)
endif()
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  list(APPEND ACTIVE_TAGS wayland linux icu)
endif()

file(GLOB_RECURSE all_sources RELATIVE "${TESTS_DIR}/abi" CONFIGURE_DEPENDS
     "${TESTS_DIR}/abi/*.c")
set(abi_sources)
set(registry "")
foreach(domain IN LISTS ABI_DOMAINS)
  set(domain_sources ${all_sources})
  list(FILTER domain_sources INCLUDE REGEX "^${domain}/[^/]+$")
  list(SORT domain_sources)
  foreach(relative IN LISTS domain_sources)
    get_filename_component(stem "${relative}" NAME_WE)
    string(REGEX MATCH "_([a-z0-9]+)$" tagged "${stem}")
    if(tagged
       AND CMAKE_MATCH_1 IN_LIST FILE_TAGS
       AND NOT CMAKE_MATCH_1 IN_LIST ACTIVE_TAGS)
      continue()
    endif()
    string(MAKE_C_IDENTIFIER "${domain}_${stem}" identifier)
    set(symbol "mln_test_group_${identifier}")
    set(source "${TESTS_DIR}/abi/${relative}")
    list(APPEND abi_sources "${source}")
    set_property(
      SOURCE "${source}"
      APPEND
      PROPERTY COMPILE_DEFINITIONS "MLN_TEST_GROUP_NAME=${symbol}")
    string(APPEND registry
           "MLN_TEST_REGISTRY_ENTRY(${symbol}, \"tests/native/abi/${relative}\")\n")
  endforeach()
endforeach()
set(registry_dir "${CMAKE_CURRENT_BINARY_DIR}/registry")
file(
  CONFIGURE
  OUTPUT "${registry_dir}/mln_native_test_registry.inc"
  CONTENT "${registry}")
# The harness reads the cases a build expects to fail from this file.
# maplibre-native-ffi lists the cases that mln-rs fails; against crates/mln_c,
# every case must pass, so the list is empty.
file(
  CONFIGURE
  OUTPUT "${registry_dir}/mln_native_expected_failures.inc"
  CONTENT "")

include(FetchContent)
fetchcontent_declare(
  unity
  URL https://github.com/ThrowTheSwitch/Unity/archive/refs/tags/v2.6.1.tar.gz
  URL_HASH
    SHA256=b41a66d45a6b99758fb3202ace6178177014d52fc524bf1f72687d93e9867292
  EXCLUDE_FROM_ALL)
fetchcontent_makeavailable(unity)
target_compile_definitions(unity PUBLIC UNITY_INCLUDE_CONFIG_H)
target_include_directories(unity PUBLIC "$<BUILD_INTERFACE:${SUPPORT_DIR}>")

# The GPU fixtures: a Vulkan device and the textures the suite renders into.
add_library(mln_test_graphics_objects OBJECT "${GRAPHICS_DIR}/graphics.c")
set_target_properties(
  mln_test_graphics_objects
  PROPERTIES C_STANDARD 11
             C_STANDARD_REQUIRED YES
             C_EXTENSIONS OFF
             C_VISIBILITY_PRESET hidden
             POSITION_INDEPENDENT_CODE ON)
target_include_directories(mln_test_graphics_objects
                           PUBLIC "${GRAPHICS_DIR}/include")
target_include_directories(mln_test_graphics_objects SYSTEM
                           PRIVATE "${VULKAN_INCLUDE_DIR}")
target_compile_definitions(
  mln_test_graphics_objects
  PRIVATE MLN_TEST_GRAPHICS_EXPORTS
          "MLN_TEST_GRAPHICS_VULKAN_LOADER=\"${VULKAN_LOADER_LIBRARY}\"")
if(MLN_C_TEST_BACKEND STREQUAL "opengl")
  target_compile_definitions(
    mln_test_graphics_objects
    PRIVATE "MLN_TEST_GRAPHICS_EGL_LIBRARY=\"${EGL_LIBRARY}\""
            "MLN_TEST_GRAPHICS_GLES_LIBRARY=\"${GLES_LIBRARY}\"")
endif()
target_link_libraries(mln_test_graphics_objects PUBLIC ${CMAKE_DL_LIBS})

add_executable(
  mln_native_abi_tests
  ${abi_sources}
  "${SUPPORT_DIR}/harness.c"
  "${SUPPORT_DIR}/watchdog.cpp"
  "${SUPPORT_DIR}/wait.cpp"
  "${SUPPORT_DIR}/env.c"
  "${SUPPORT_DIR}/render.c"
  "${SUPPORT_DIR}/frames.c"
  "${SUPPORT_DIR}/camera.c"
  "${SUPPORT_DIR}/map.c"
  "${SUPPORT_DIR}/style.c"
  "${SUPPORT_DIR}/resources.c"
  "${SUPPORT_DIR}/render_graphics.c"
  "${SUPPORT_DIR}/http_server.c")
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  target_sources(mln_native_abi_tests
                 PRIVATE "${SUPPORT_DIR}/wayland_compositor.c")
endif()
set_target_properties(
  mln_native_abi_tests
  PROPERTIES C_STANDARD 23
             C_STANDARD_REQUIRED YES
             C_EXTENSIONS OFF
             CXX_STANDARD 23
             CXX_STANDARD_REQUIRED YES
             CXX_EXTENSIONS OFF)
target_include_directories(
  mln_native_abi_tests PRIVATE "${TESTS_DIR}" "${SUPPORT_DIR}" "${registry_dir}"
                               "${VULKAN_INCLUDE_DIR}")
string(TOUPPER "${MLN_C_TEST_BACKEND}" backend)
target_compile_definitions(mln_native_abi_tests
                           PRIVATE MLN_FFI_TEST_BACKEND_${backend}=1)
if(MLN_C_TEST_BACKEND STREQUAL "opengl")
  target_compile_definitions(mln_native_abi_tests
                             PRIVATE MLN_FFI_TEST_OPENGL_EGL=1)
  target_link_libraries(mln_native_abi_tests PRIVATE "${EGL_LIBRARY}"
                                                     "${GLES_LIBRARY}")
endif()
target_compile_options(
  mln_native_abi_tests
  PRIVATE -Werror=unused-function
          $<$<COMPILE_LANGUAGE:C>:-Werror=missing-prototypes>)
target_link_libraries(
  mln_native_abi_tests
  PRIVATE unity::framework
          maplibre_native_c
          mln_test_graphics_objects
          "${VULKAN_LOADER_LIBRARY}"
          Threads::Threads
          ${CMAKE_DL_LIBS})
target_link_options(mln_native_abi_tests PRIVATE "LINKER:--allow-shlib-undefined")

# The exit programs (testdata/tests/native/exit), one executable each in exit/,
# as maplibre-native-ffi's mln_native_add_exit_tests builds them. Each makes
# its calls and returns from main while the library's threads are at work, so
# the process exit that follows is the check: run.py expects each to exit with
# status 0. A program whose name ends in a tag builds only where the tag is
# active, as a suite file does.
file(GLOB exit_programs RELATIVE "${TESTS_DIR}/exit" CONFIGURE_DEPENDS
     "${TESTS_DIR}/exit/*.c")
foreach(program IN LISTS exit_programs)
  get_filename_component(stem "${program}" NAME_WE)
  string(REGEX MATCH "_([a-z0-9]+)$" tagged "${stem}")
  if(tagged
     AND CMAKE_MATCH_1 IN_LIST FILE_TAGS
     AND NOT CMAKE_MATCH_1 IN_LIST ACTIVE_TAGS)
    continue()
  endif()
  set(target "mln_native_exit_${stem}")
  add_executable(${target} "${TESTS_DIR}/exit/${program}")
  set_target_properties(
    ${target}
    PROPERTIES C_STANDARD 11
               C_STANDARD_REQUIRED YES
               OUTPUT_NAME "${stem}"
               RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/exit")
  target_link_libraries(${target} PRIVATE maplibre_native_c
                                          mln_test_graphics_objects Threads::Threads)
  target_link_options(${target} PRIVATE "LINKER:--allow-shlib-undefined")
endforeach()
