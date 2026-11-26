//! Each scenario runs in a process of its own: the runner starts a binary of
//! the harness with `exec <scenario> <out>`, and reads the [`Run`] it writes.
//! No scenario inherits another's heap, threads or caches.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::environment::Environment;
use crate::meter;
use crate::report::Run;
use crate::scenario::{Bench, Scenario};

/// Runs `scenario` in a process of `binary`, which must be a harness binary
/// built from the same source to give the same scenario. With `image`, the
/// scenario writes the last image it drew there, as a PNG.
pub(crate) fn run_in_process(binary: &Path, scenario: &str, image: Option<&Path>) -> Result<Run, String> {
    let out = tempfile::NamedTempFile::new().map_err(|error| format!("a temporary file: {error}"))?;
    let mut command = Command::new(binary);
    command.args(["exec", scenario]).arg(out.path());
    if let Some(image) = image {
        command.arg("--image").arg(image);
    }
    // The scenario's own output goes to stderr, so that stdout carries only
    // the report.
    let status = command
        .stdin(Stdio::null())
        .stdout(std::io::stderr())
        .status()
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    if !status.success() {
        return Err(format!("{scenario} failed ({status})"));
    }
    let json = std::fs::read_to_string(out.path()).map_err(|error| format!("{scenario}'s report: {error}"))?;
    serde_json::from_str(&json).map_err(|error| format!("{scenario}'s report: {error}"))
}

/// Runs each of `scenarios` once in a process of `binary`, and returns what
/// they report, which no measurement counts. The first process of a newly
/// built binary to run some code does more work than later ones: on macOS
/// the system checks each page of the binary's code the first time a
/// process reads it, and caches the GPU code it compiles for each binary.
pub(crate) fn warm_up(binary: &Path, scenarios: &[&Scenario]) -> Result<Vec<Run>, String> {
    scenarios.iter().map(|scenario| run_in_process(binary, scenario.name, None)).collect()
}

/// Fails unless this binary was built as `profile.toml` builds it, with the
/// libraries' hash maps seeded alike in every process.
pub(crate) fn check_build() -> Result<(), String> {
    if mln_style::hash::SEED_IS_FIXED {
        Ok(())
    } else {
        Err("this build lacks tools/bench/profile.toml's `--cfg mln_bench_fixed_hash`, without which hash maps \
             hash differently in every run: build with `--config tools/bench/profile.toml`, and without RUSTFLAGS"
            .to_owned())
    }
}

/// Runs `scenario` in this process and writes its [`Run`] to `out`: what the
/// runner's child process does.
pub(crate) fn exec(scenario: &Scenario, out: &Path, image: Option<PathBuf>) -> Result<(), String> {
    check_build()?;
    // The pipeline statistics and time of the render passes, of the
    // devices the scenario makes.
    meter::measure_render_passes();
    let mut bench = Bench::new();
    bench.image = image;
    (scenario.run)(&mut bench);
    let gpu = bench.gpu.take();
    let fixtures = bench.fixtures.take();
    let metrics = bench.finish();
    let run = Run {
        scenario: scenario.name.to_owned(),
        kind: scenario.kind,
        harness: env!("MLN_BENCH_HARNESS").to_owned(),
        environment: Environment { gpu, fixtures, ..Environment::machine() },
        metrics,
    };
    let json = serde_json::to_string(&run).map_err(|error| error.to_string())?;
    std::fs::write(out, json).map_err(|error| format!("{}: {error}", out.display()))
}
