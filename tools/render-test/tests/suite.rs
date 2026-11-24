// Runs the runner as src/main.rs would: from the package directory (where
// `cargo test` runs the tests), with the manifest for the OS and backend, and
// the HTML report and the actual/diff images in target/tmp/<name>. Fill
// outlines are 2 px wide, as C++ draws them on OpenGL, so OpenGL, Vulkan and
// WebGPU take C++'s OpenGL or Vulkan expectations; WebGPU on a CPU
// rasterizer and D3D12 draw them 1 px wide (ignores/rust-port-1px-outlines.json):
// D3D12 has no wide lines, and takes WebGPU's manifest.
// macOS adds the C++ WebGPU ignore list, since wgpu renders through Metal
// there, and ignores the tests whose only expected image is C++ Metal's, which
// triangulates them 1 px wide (ignores/rust-port-metal-outlines.json).
//
// The tests run in worker processes of the package's render-test binary
// (`--jobs`, src/jobs.rs): one per CPU, or $RENDER_TEST_JOBS (1: in the
// test's process). A device runs the test from a copy of its package, without
// that binary, so there they run in the test's process, and write to the
// directory that $MLN_RENDER_TEST_OUTPUT names, since the build's
// CARGO_TARGET_TMPDIR is on another machine.

use std::path::{Path, PathBuf};

use render_test::render_test::{parse_arguments, run_render_tests_with};
use render_test::runner::ImplFactory;

/// The runner's exit code for `args` (after `--manifestPath`, `--output`
/// and `--jobs`).
pub(crate) fn run(name: &str, args: &[&str], factory: Option<ImplFactory>) -> i32 {
    let manifest = if cfg!(target_os = "macos") {
        "manifests/macos-wgpu.json"
    } else {
        match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
            Ok("gl" | "gl-desktop") => "manifests/linux-opengl.json",
            Ok("vulkan") => "manifests/linux-vulkan.json",
            Ok("d3d12") => "manifests/windows-d3d12.json",
            _ => "manifests/linux-wgpu.json",
        }
    };
    let output = std::env::var_os("MLN_RENDER_TEST_OUTPUT")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from)
        .join(name);
    let worker = Path::new(env!("CARGO_BIN_EXE_render-test"));
    let jobs =
        std::env::var("RENDER_TEST_JOBS").unwrap_or_else(|_| if worker.exists() { "0" } else { "1" }.to_string());
    let mut argv =
        vec!["render-test", "--manifestPath", manifest, "--output", output.to_str().unwrap(), "--jobs", &jobs];
    argv.extend(args);
    match parse_arguments(&argv.iter().map(ToString::to_string).collect::<Vec<_>>()) {
        Ok(mut arguments) => {
            arguments.worker_program = Some(worker.to_path_buf());
            run_render_tests_with(&arguments, factory).return_code
        }
        Err(code) => code,
    }
}
