// Compiles every program to SPIR-V in both projection variants.
mod spirv_test;
// The window backend on a headless surface (VK_EXT_headless_surface): frames
// through the swapchain, resizes, the overlay, readback, two frames in
// flight.
mod surface_test;
// mln_engine::conformance's frame through `VulkanGpu`, and its shaders
// and texture writes.
mod gpu_test;
// A library device logs the GPU it is on, as maplibre-native-ffi's graphics
// devices do.
mod log_test;
// A borrowed ring's checks of its images, and what its construction leaves
// behind when it fails.
mod borrowed_target_test;

/// Whether `MLN_RENDER_BACKEND` asks for Vulkan.
fn vulkan_requested() -> bool {
    std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("vulkan"))
}

/// A Vulkan device, or `None` where no Vulkan ICD is available: the test is
/// skipped there, unless `MLN_RENDER_BACKEND` asks for `vulkan`.
fn device() -> Option<crate::Device> {
    match crate::Device::new(None) {
        Ok(device) => Some(device),
        Err(error) if error.kind == mln_gfx::ErrorKind::Unsupported && !vulkan_requested() => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("Vulkan device: {error}"),
    }
}
