mod borrowed_target_test;
mod gpu_test;
mod shader_group_preprocessor;
mod shared_device_test;
mod wgsl_compile_test;
// A library device logs the adapter it is on, as maplibre-native-ffi's
// graphics devices log theirs.
mod log_test;

/// Whether `MLN_RENDER_BACKEND` asks for WebGPU.
pub(crate) fn webgpu_requested() -> bool {
    std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("webgpu"))
}

/// A WebGPU device, or `None` where no adapter is available: the test is
/// skipped there, unless `MLN_RENDER_BACKEND` asks for `webgpu`.
pub(crate) fn device() -> Option<crate::Device> {
    match crate::Device::new(None) {
        Ok(device) => Some(device),
        Err(error) if error.kind == mln_util::ErrorKind::Unsupported && !webgpu_requested() => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("WebGPU device: {error}"),
    }
}
