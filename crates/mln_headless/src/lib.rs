//! Rendering without a window: `HeadlessFrontend`, ported from C++'s
//! `platform/default`.

pub mod headless_frontend;

pub use headless_frontend::{HeadlessFrontend, RenderResult, harness_device};

/// Prints the warnings and errors of the library, and of the crates it
/// uses, to stderr.
struct StderrLog;

impl log::Log for StderrLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] [{}] {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Makes the program's `log` logger print warnings and errors to stderr,
/// unless it has a logger already: for the tools that render with
/// [`HeadlessFrontend`].
pub fn log_to_stderr() {
    if log::set_logger(&StderrLog).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

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
