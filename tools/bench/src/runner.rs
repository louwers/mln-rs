//! Each scenario runs in a process of its own: the runner starts a binary of
//! the harness with `exec <scenario> <out>`, and reads the [`Run`] it writes.
//! No scenario inherits another's heap, threads or caches.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::environment::Environment;
use crate::report::Run;
use crate::scenario::{Bench, Scenario};

/// Runs `scenario` in a process of `binary`, which must be a harness binary
/// built from the same source to give the same scenario.
pub(crate) fn run_in_process(binary: &Path, scenario: &str) -> Result<Run, String> {
    let out = tempfile::NamedTempFile::new().map_err(|error| format!("a temporary file: {error}"))?;
    // The scenario's own output goes to stderr, so that stdout carries only
    // the report.
    let status = Command::new(binary)
        .args(["exec", scenario])
        .arg(out.path())
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

/// Runs `scenario` in this process and writes its [`Run`] to `out`: what the
/// runner's child process does.
pub(crate) fn exec(scenario: &Scenario, out: &Path) -> Result<(), String> {
    let mut bench = Bench::new();
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
