//! Ports `expression-test/` of the C++ repo: runs
//! `metrics/integration/expression-tests` with the C++ runner's rules.

pub mod parser;
pub mod runner;
pub mod test_runner_common;

use std::path::{Path, PathBuf};

use parser::{collect_test_paths, parse_expression_ignores, parse_test_data};
use runner::{TestRunOutput, TestStats, run_expression_test};

/// The copy of the C++ repository's metrics directory (testdata/metrics):
/// the expression tests and the ignore list they use.
pub fn find_metrics_dir() -> Option<PathBuf> {
    let metrics = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/metrics");
    metrics.join("integration/expression-tests").exists().then_some(metrics)
}

/// Runs the tests (the loop of expression-test/main.cpp). `verbose` prints
/// each result like the C++ runner.
pub fn run(metrics: &Path, test_names: &[String], filter: Option<&str>, verbose: bool) -> TestStats {
    let root_path = metrics.join("integration/expression-tests");
    let test_paths = collect_test_paths(&root_path, test_names, filter);
    let ignores = parse_expression_ignores(&metrics.join("ignores/platform-all.json"));

    let mut stats = TestStats::default();
    for path in &test_paths {
        let expectation = path.parent().expect("test directory");
        // The parts joined with `/` on every platform, as the ignore list
        // names its tests.
        let relative = expectation.strip_prefix(&root_path).expect("test under the root");
        let id = relative.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/");
        stats.ids.push(id.clone());

        let ignore_name = format!("expression-tests/{id}");
        let ignore = ignores.iter().find(|ignore| ignore.id == ignore_name);

        let test_run = match parse_test_data(path) {
            Ok(mut data) => {
                // A panic is reported as an error for this test (C++ asserts
                // abort the whole run).
                let id2 = id.clone();
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || run_expression_test(&mut data, &id2)))
                    .map_err(|e| {
                        e.downcast_ref::<String>()
                            .cloned()
                            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                            .unwrap_or_default()
                    })
            }
            Err(e) => Err(e),
        };

        let test_run = match test_run {
            Ok(r) => r,
            Err(e) => {
                let output = TestRunOutput { id: id.clone(), text: e, ..Default::default() };
                if ignore.is_some() {
                    stats.ignore_failed.push(output);
                } else {
                    stats.errored.push(output);
                }
                if verbose {
                    println!("* ERROR can't parse '{id}' test");
                }
                continue;
            }
        };

        if let Some(ignore) = ignore {
            if test_run.passed {
                if verbose {
                    println!("* PASSED ignored test {id} ({})", ignore.reason);
                }
                stats.ignore_passed.push(test_run);
            } else {
                if verbose {
                    println!("* FAILED ignored test {id} ({})", ignore.reason);
                }
                stats.ignore_failed.push(test_run);
            }
        } else if test_run.passed {
            if verbose {
                println!("* PASSED {id}");
            }
            stats.passed.push(test_run);
        } else {
            if verbose {
                print!("* FAILED {id}\n{}", test_run.text);
            }
            stats.failed.push(test_run);
        }
    }
    stats
}

/// `printStats` (expression_test_logger.cpp).
pub fn print_stats(stats: &TestStats) {
    let count = stats.test_count();
    let pct = |n: usize| if count == 0 { 0.0 } else { 100.0 * n as f64 / count as f64 };
    if !stats.passed.is_empty() {
        println!("{} passed ({:.1}%)", stats.passed.len(), pct(stats.passed.len()));
    }
    if !stats.ignore_passed.is_empty() {
        println!("{} passed but were ignored ({:.1}%)", stats.ignore_passed.len(), pct(stats.ignore_passed.len()));
    }
    if !stats.ignore_failed.is_empty() {
        println!("{} ignored ({:.1}%)", stats.ignore_failed.len(), pct(stats.ignore_failed.len()));
    }
    if !stats.failed.is_empty() {
        println!("{} failed ({:.1}%)", stats.failed.len(), pct(stats.failed.len()));
    }
    if !stats.errored.is_empty() {
        println!("{} errored ({:.1}%)", stats.errored.len(), pct(stats.errored.len()));
    }
}
