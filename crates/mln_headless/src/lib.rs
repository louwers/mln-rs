//! Rendering without a window: `HeadlessFrontend`, ported from C++'s
//! `platform/default`.

pub mod headless_frontend;

pub use headless_frontend::{HeadlessFrontend, RenderResult, harness_device};

/// The directory that a test writes its images to: the one that
/// `MLN_TEST_OUTPUT` names, a temporary directory on devices whose tests run
/// from a copy of their package (Android and `OpenHarmony`), or else
/// `target_tmpdir`, the test's `CARGO_TARGET_TMPDIR`, which is on the machine
/// that built it.
#[must_use]
pub fn test_output_dir(target_tmpdir: &str) -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("MLN_TEST_OUTPUT") {
        return dir.into();
    }
    if cfg!(any(target_os = "android", target_env = "ohos")) {
        // `CARGO_TARGET_TMPDIR` is a host path on a device whose root
        // filesystem is read-only; prefer `TMPDIR` or `/data/local/tmp`.
        let base = std::env::var_os("TMPDIR")
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| std::path::PathBuf::from("/data/local/tmp"), std::path::PathBuf::from);
        return base.join("mln-test-output");
    }
    target_tmpdir.into()
}
