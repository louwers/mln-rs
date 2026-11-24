//! Rendering without a window: `HeadlessFrontend`, ported from C++'s
//! `platform/default`.

pub mod headless_frontend;

pub use headless_frontend::{HeadlessFrontend, RenderResult, harness_device};

/// The directory that a test writes its images to: the one that
/// `MLN_TEST_OUTPUT` names, which a device that runs the test from a copy of
/// its package sets, or else `target_tmpdir`, the test's
/// `CARGO_TARGET_TMPDIR`, which is on the machine that built it.
#[must_use]
pub fn test_output_dir(target_tmpdir: &str) -> std::path::PathBuf {
    std::env::var_os("MLN_TEST_OUTPUT").map_or_else(|| target_tmpdir.into(), Into::into)
}
