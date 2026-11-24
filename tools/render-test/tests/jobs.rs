// The runner's `--jobs` (src/jobs.rs) over a part of the shared suite, with
// `--self-check` (no rendering): the same results and HTML report as a run
// in one process, and errored tests when no worker runs.

use std::fs;
use std::path::{Path, PathBuf};

use render_test::render_test::{RunResult, parse_arguments, run_render_tests_with};

// Tests that run, ignored ones and skipped ones ("skip - js specific").
const FILTER: &str = "/render-tests/(background-color|canvas|line-pattern)/";

/// The run's result, its HTML report and its output directory.
fn self_check(name: &str, jobs: usize, worker_program: &Path) -> (RunResult, String, PathBuf) {
    let package_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    std::env::set_current_dir(&package_dir).unwrap_or_else(|e| panic!("chdir {}: {e}", package_dir.display()));
    let manifest =
        if cfg!(target_vendor = "apple") { "manifests/macos-wgpu.json" } else { "manifests/linux-wgpu.json" };
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join(name);
    let args = [
        "render-test",
        "--manifestPath",
        manifest,
        "--output",
        output.to_str().unwrap(),
        "--self-check",
        "--filter",
        FILTER,
        &format!("--jobs={jobs}"),
    ];
    let mut arguments = parse_arguments(&args.map(String::from)).unwrap();
    arguments.worker_program = Some(worker_program.to_path_buf());
    let result = run_render_tests_with(&arguments, None);
    let stem = Path::new(manifest).file_stem().unwrap().to_str().unwrap();
    let report = fs::read_to_string(output.join(format!("{stem}.html"))).unwrap();
    (result, report, output)
}

fn ids(result: &RunResult) -> Vec<(&str, &str)> {
    result.metadatas.iter().map(|metadata| (metadata.id.as_str(), metadata.status.as_str())).collect()
}

#[test]
fn jobs_match_a_run_in_one_process() {
    let worker = Path::new(env!("CARGO_BIN_EXE_render-test"));
    let (sequential, sequential_report, _) = self_check("jobs_sequential", 1, worker);
    let (parallel, parallel_report, output) = self_check("jobs_parallel", 3, worker);

    assert_eq!(sequential.return_code, 0);
    assert!(sequential.stats.passed_tests > 0 && sequential.stats.ignore_passed_tests > 0);
    assert!(!ids(&sequential).iter().any(|(id, _)| id.starts_with("render-tests/canvas/")));

    assert_eq!(parallel.return_code, sequential.return_code);
    assert_eq!(parallel.stats, sequential.stats);
    assert_eq!(ids(&parallel), ids(&sequential));
    assert!(parallel_report == sequential_report, "the reports differ");
    assert!(!output.join(".render-test-jobs").exists());
}

#[test]
fn every_test_errors_without_workers() {
    let (result, report, _) = self_check("jobs_no_worker", 2, Path::new("/nonexistent/render-test"));

    assert_eq!(result.return_code, 2);
    assert!(result.stats.errored_tests > 0);
    for metadata in &result.metadatas {
        assert!(matches!(metadata.status.as_str(), "errored" | "ignored failed"), "{}", metadata.id);
        assert!(metadata.error_message.starts_with("No worker ran this test"), "{}", metadata.error_message);
    }
    assert!(report.contains("No worker ran this test"));
}
