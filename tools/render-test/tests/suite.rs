// Runs the runner as src/main.rs would: from the package directory (where
// `cargo test` runs the tests), with the manifest for the OS and backend, and
// the HTML report and the actual/diff images in <name> in the test output
// directory (mln_headless::test_output_dir). Fill
// outlines are 2 px wide, as C++ draws them on OpenGL, so OpenGL, Vulkan and
// WebGPU take C++'s OpenGL or Vulkan expectations; WebGPU on a CPU
// rasterizer and D3D12 draw them 1 px wide (ignores/rust-port-1px-outlines.json):
// D3D12 has no wide lines, and takes WebGPU's manifest.
// macOS adds the C++ WebGPU ignore list, since wgpu renders through Metal
// there, and ignores the tests whose only expected image is C++ Metal's, which
// triangulates them 1 px wide (ignores/rust-port-metal-outlines.json).
// Android's Vulkan manifest adds what Mali GPUs draw differently
// (ignores/rust-port-mali.json), as the Pixel 9 that CI tests on has.
//
// The tests run in worker processes of the package's render-test binary
// (`--jobs`, src/jobs.rs): one per CPU, or $MLN_RENDER_TEST_JOBS (1: in the
// test's process), which the target environments whose devices run a test
// from a copy of its package set to 1, since the copy lacks that binary.

use std::path::PathBuf;

use render_test::render_test::{parse_arguments, run_render_tests_with};
use render_test::runner::ImplFactory;

/// The runner's exit code for `args` (after `--manifestPath`, `--output`
/// and `--jobs`).
pub(crate) fn run(name: &str, args: &[&str], factory: Option<ImplFactory>) -> i32 {
    let package_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    std::env::set_current_dir(&package_dir).unwrap_or_else(|e| panic!("chdir {}: {e}", package_dir.display()));
    let manifest = if cfg!(target_vendor = "apple") {
        "manifests/macos-wgpu.json"
    } else {
        match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
            Ok("gl" | "gl-desktop") => "manifests/linux-opengl.json",
            Ok("vulkan") if cfg!(target_os = "android") => "manifests/android-vulkan.json",
            Ok("vulkan") => "manifests/linux-vulkan.json",
            Ok("d3d12") => "manifests/windows-d3d12.json",
            _ => "manifests/linux-wgpu.json",
        }
    };
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join(name);
    let jobs = std::env::var("MLN_RENDER_TEST_JOBS").unwrap_or_else(|_| "0".to_string());
    let mut argv =
        vec!["render-test", "--manifestPath", manifest, "--output", output.to_str().unwrap(), "--jobs", &jobs];
    argv.extend(args);
    match parse_arguments(&argv.iter().map(ToString::to_string).collect::<Vec<_>>()) {
        Ok(mut arguments) => {
            arguments.worker_program = Some(PathBuf::from(env!("CARGO_BIN_EXE_render-test")));
            run_render_tests_with(&arguments, factory).return_code
        }
        Err(code) => code,
    }
}
