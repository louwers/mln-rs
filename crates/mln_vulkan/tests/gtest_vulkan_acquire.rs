// Ports test/renderer/vulkan_acquire.test.cpp (built on Vulkan only in C++,
// `#if MLN_RENDER_BACKEND_VULKAN`).
//
// The tests drive `vulkan::Context::beginFrame`'s swapchain acquisition
// (timeouts, out-of-date swapchains, lost surfaces) on a headless backend,
// by replacing entries of the backend's Vulkan dispatcher with stubs. The
// port calls Vulkan through ash's function tables, which can't be replaced,
// and has no `SurfaceNotReady`: acquiring an image waits (context.rs).

// gtest: VulkanContext.AcquireTimeoutAbortsFrame
#[test]
#[ignore = "needs C++'s replaceable Vulkan dispatcher, and the acquire timeout the port doesn't have"]
fn acquire_timeout_aborts_frame() {
    // test/renderer/vulkan_acquire.test.cpp:138
}

// gtest: VulkanContext.FailedSwapchainRebuildIsRetried
#[test]
#[ignore = "needs C++'s replaceable Vulkan dispatcher, to fail a swapchain rebuild"]
fn failed_swapchain_rebuild_is_retried() {
    // test/renderer/vulkan_acquire.test.cpp:171
}

// gtest: VulkanContext.FailedSurfaceRecreationIsRetried
#[test]
#[ignore = "needs C++'s replaceable Vulkan dispatcher, to lose the surface"]
fn failed_surface_recreation_is_retried() {
    // test/renderer/vulkan_acquire.test.cpp:189
}
