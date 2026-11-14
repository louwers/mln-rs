// Runs the runner as src/main.rs would: from the package directory (where
// `cargo test` runs the tests), with the manifest for the OS and backend, and
// the HTML report and the actual/diff images in target/tmp/<name>. macOS adds
// the C++ WebGPU ignore list, since wgpu renders through Metal there. OpenGL
// and Vulkan take C++'s expectations and ignores for those backends, which
// draw fill outlines as lines where Metal and WebGPU draw triangles.
//
// The tests run in worker processes of the package's render-test binary
// (`--jobs`, src/jobs.rs): one per CPU, or $RENDER_TEST_JOBS (1: in the
// test's process).

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
            _ => "manifests/linux-wgpu.json",
        }
    };
    let output = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let jobs = std::env::var("RENDER_TEST_JOBS").unwrap_or_else(|_| "0".to_string());
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
