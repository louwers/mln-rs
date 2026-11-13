// `--jobs=<n>`: the runner's tests in n worker processes.
//
// A test runs on one thread, and most of a test's time is spent there (on
// lavapipe, largely compiling the shaders for the test's new device), so
// the suite runs about as many times faster as there are workers, up to
// the number of CPUs.
//
// The runner process (the parent) writes the run's arguments and its list
// of tests to a work directory and starts n workers,
// `<worker program> --render-test-worker <dir>`. The list is passed on
// because the manifest lists the tests in `readdir` order (as C++ does),
// which two processes need not see alike. A worker takes the first test
// that no worker has taken by creating `<dir>/claims/<index>` (only one
// process can create it), runs the test as the sequential runner does
// (`run_test`) and writes its result to `<dir>/results/<index>`, until every
// test is taken.
//
// The parent relays the workers' output (with `print!`, so a test harness
// captures it as it captures the runner's own) and then reads the results
// in test order: the statistics, the return code and the HTML report are
// those of a sequential run, while the per-test lines are printed in the
// order the tests finish. A worker that dies (an abort, a signal) errors the
// test it was running, and the parent starts another worker in its place.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use mln_util::size::Size;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::file_source::ProxyFileSource;
use crate::log::{self, Event};
use crate::manifest_parser::{Manifest, ManifestParser};
use crate::metadata::{TestMetadata, TestPaths};
use crate::render_test::{Arguments, create_runner, finish_test, prepare_test, run_test};
use crate::runner::ImplFactory;

/// The hidden option that makes `render-test` a worker:
/// `render-test --render-test-worker <dir>`.
pub const WORKER_FLAG: &str = "--render-test-worker";

/// In the work directory: the run (JSON), the taken tests (`<index>`, each
/// holding the process id of the worker that took it) and the results.
const RUN_FILE: &str = "run.json";
const CLAIMS_DIR: &str = "claims";
const RESULTS_DIR: &str = "results";

/// The number of workers for `--jobs=<requested>` and `tests` tests: one per
/// CPU for 0, and no more than there are tests. Below 2 the tests run in the
/// runner's process.
pub fn job_count(requested: usize, tests: usize) -> usize {
    let jobs =
        if requested == 0 { std::thread::available_parallelism().map_or(1, NonZeroUsize::get) } else { requested };
    jobs.min(tests)
}

/// What the workers of a run share.
#[derive(Serialize, Deserialize)]
struct Run {
    arguments: Arguments,
    /// The parent's tests, in its order.
    tests: Vec<TestPaths>,
}

/// Runs the tests of `manifest` in `jobs` workers. Returns the metadata of
/// every test that was not skipped, in test order. The metadata of a test
/// that a worker ran holds what the statistics, the return code and the
/// report read (`ResultHeader`).
#[must_use]
pub fn run_jobs(arguments: &Arguments, manifest: &Manifest, output_path: &Path, jobs: usize) -> Vec<TestMetadata> {
    let dir = output_path.join(".render-test-jobs");
    let (mut crashes, not_run) = match start(arguments, manifest, &dir) {
        Ok(program) => (
            supervise(&program, &dir, jobs),
            "No worker ran this test: the workers exited (see the errors above)".to_string(),
        ),
        Err(e) => {
            log::error(Event::General, &format!("Failed to prepare the render-test workers in {}: {e}", dir.display()));
            (BTreeMap::new(), format!("No worker ran this test: {e}"))
        }
    };

    let ignores = manifest.get_ignores();
    let root_length = manifest.get_test_root_path().len();
    let mut metadatas = Vec::new();
    for (index, test_path) in manifest.get_test_paths().iter().enumerate() {
        let error = match read_result(&dir, index, test_path) {
            Ok(WorkerResult::Finished(metadata)) => {
                metadatas.push(*metadata);
                continue;
            }
            Ok(WorkerResult::Skipped) => continue,
            Ok(WorkerResult::Missing) => crashes.remove(&index).unwrap_or_else(|| not_run.clone()),
            Err(e) => format!("Failed to read this test's result from its worker: {e}"),
        };
        // Reported as a test that errored before it rendered.
        if let Some((mut metadata, ignore_reason)) = prepare_test(test_path, ignores, root_length) {
            metadata.error_message = error;
            metadata.render_errored += 1;
            finish_test(&mut metadata, ignore_reason.as_deref(), true, false);
            metadatas.push(metadata);
        }
    }

    if let Err(e) = fs::remove_dir_all(&dir) {
        log::warning(Event::General, &format!("Failed to remove {}: {e}", dir.display()));
    }
    metadatas
}

/// Prepares the work directory `dir` and returns the program the workers
/// run.
fn start(arguments: &Arguments, manifest: &Manifest, dir: &Path) -> io::Result<PathBuf> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    fs::create_dir_all(dir.join(CLAIMS_DIR))?;
    fs::create_dir_all(dir.join(RESULTS_DIR))?;
    let run = Run { arguments: arguments.clone(), tests: manifest.get_test_paths().to_vec() };
    fs::write(dir.join(RUN_FILE), serde_json::to_vec(&run)?)?;
    arguments.worker_program.clone().map_or_else(std::env::current_exe, Ok)
}

/// Starts `jobs` workers and waits for them. A worker that exits
/// unsuccessfully while it runs a test is replaced. Returns the tests the
/// workers were running when they exited, with how they exited.
fn supervise(program: &Path, dir: &Path, jobs: usize) -> BTreeMap<usize, String> {
    let crashes = Mutex::new(BTreeMap::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let (status, pid) = match run_worker_process(program, dir) {
                        Ok(exited) => exited,
                        Err(e) => {
                            log::error(
                                Event::General,
                                &format!("Failed to run the render-test worker {}: {e}", program.display()),
                            );
                            return;
                        }
                    };
                    if status.success() {
                        return;
                    }
                    let running = unfinished_claims(dir, pid);
                    if running.is_empty() {
                        // Not replaced: a worker that fails before it takes a
                        // test would fail again.
                        log::error(Event::General, &format!("A render-test worker exited ({status})"));
                        return;
                    }
                    let mut crashes = crashes.lock();
                    for index in running {
                        crashes.insert(index, format!("The worker running this test exited ({status})"));
                    }
                }
            });
        }
    });
    crashes.into_inner()
}

/// Runs one worker to its end, relaying its standard output and error.
/// Returns how it exited and its process id.
fn run_worker_process(program: &Path, dir: &Path) -> io::Result<(ExitStatus, u32)> {
    let mut child = Command::new(program)
        .arg(WORKER_FLAG)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    std::thread::scope(|scope| {
        scope.spawn(|| relay(stderr, |line| eprint!("{line}")));
        relay(stdout, |line| print!("{line}"));
    });
    Ok((child.wait()?, pid))
}

/// Writes each line of `stream` with `write` until the stream ends.
fn relay(stream: impl Read, mut write: impl FnMut(&str)) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).is_ok_and(|read| read > 0) {
        write(&String::from_utf8_lossy(&line));
        line.clear();
    }
}

/// The tests that the worker with process id `pid` took and did not finish.
fn unfinished_claims(dir: &Path, pid: u32) -> Vec<usize> {
    let Ok(entries) = fs::read_dir(dir.join(CLAIMS_DIR)) else {
        return Vec::new();
    };
    let pid = pid.to_string();
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let index: usize = entry.file_name().to_str()?.parse().ok()?;
            let finished = dir.join(RESULTS_DIR).join(index.to_string()).exists();
            (!finished && fs::read_to_string(entry.path()).ok()? == pid).then_some(index)
        })
        .collect()
}

/// `render-test --render-test-worker <dir>`: runs the tests of the run in
/// `dir` that no other worker has taken, until every test is taken. 0 when
/// no test is left.
pub fn run_worker(dir: &Path, factory: Option<ImplFactory>) -> i32 {
    let run: Run = match fs::read(dir.join(RUN_FILE)).and_then(|bytes| Ok(serde_json::from_slice(&bytes)?)) {
        Ok(run) => run,
        Err(e) => {
            log::error(Event::General, &format!("Failed to read {}: {e}", dir.join(RUN_FILE).display()));
            return 1;
        }
    };
    let arguments = &run.arguments;

    // As runRenderTests prepares its run. The tests are the parent's.
    ProxyFileSource::set_offline(!arguments.online);
    let Some(manifest) = ManifestParser::parse_manifest(&arguments.manifest_path, arguments.test_filter.clone()) else {
        return 5;
    };
    let Some(output_path) = arguments.output.as_deref().map(PathBuf::from) else {
        log::error(Event::General, "No output directory: pass --output <dir>");
        return 1;
    };
    let ignores = manifest.get_ignores().to_vec();
    let root_length = manifest.get_test_root_path().len();
    let mut runner = create_runner(arguments, manifest, factory, output_path);

    for (index, test_path) in run.tests.iter().enumerate() {
        match claim(dir, index) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                log::error(Event::General, &format!("Failed to take test {index}: {e}"));
                return 1;
            }
        }
        let metadata = run_test(&mut runner, test_path, &ignores, root_length, arguments.recycle_map);
        if let Err(e) = write_result(dir, index, metadata.as_ref()) {
            log::error(Event::General, &format!("Failed to write the result of test {index}: {e}"));
            return 1;
        }
    }
    0
}

/// Takes test `index` unless another worker has: true when this one did.
fn claim(dir: &Path, index: usize) -> io::Result<bool> {
    let path = dir.join(CLAIMS_DIR).join(index.to_string());
    match fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(std::process::id().to_string().as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// A test's result as a worker writes it: a JSON line, `null` for a skipped
/// test, else this header, followed by the actual, expected and diff bytes.
/// The header holds the metadata that `tally` and `create_result_page` read.
#[derive(Serialize, Deserialize)]
struct ResultHeader {
    id: String,
    status: String,
    color: String,
    ignored_test: bool,
    render_test: bool,
    outputs_image: bool,
    width: u32,
    height: u32,
    metrics_errored: u32,
    metrics_failed: u32,
    render_errored: u32,
    render_failed: u32,
    label_cut_off_found: u32,
    error_message: String,
    /// `difference.to_bits()`: exact, and JSON has no NaN.
    difference: u64,
    actual_len: usize,
    expected_len: usize,
    diff_len: usize,
}

/// Writes the result of test `index` (`None`: skipped).
fn write_result(dir: &Path, index: usize, metadata: Option<&TestMetadata>) -> io::Result<()> {
    let header = metadata.map(|m| ResultHeader {
        id: m.id.clone(),
        status: m.status.clone(),
        color: m.color.clone(),
        ignored_test: m.ignored_test,
        render_test: m.render_test,
        outputs_image: m.outputs_image,
        width: m.size.width,
        height: m.size.height,
        metrics_errored: m.metrics_errored,
        metrics_failed: m.metrics_failed,
        render_errored: m.render_errored,
        render_failed: m.render_failed,
        label_cut_off_found: m.label_cut_off_found,
        error_message: m.error_message.clone(),
        difference: m.difference.to_bits(),
        actual_len: m.actual.len(),
        expected_len: m.expected.len(),
        diff_len: m.diff.len(),
    });
    let mut bytes = serde_json::to_vec(&header)?;
    bytes.push(b'\n');
    if let Some(m) = metadata {
        bytes.extend_from_slice(&m.actual);
        bytes.extend_from_slice(&m.expected);
        bytes.extend_from_slice(&m.diff);
    }
    // Renamed into place, so that a worker that dies while writing leaves no
    // result.
    let path = dir.join(RESULTS_DIR).join(index.to_string());
    let partial = path.with_extension("partial");
    fs::write(&partial, bytes)?;
    fs::rename(partial, path)
}

/// What the workers left for one test.
enum WorkerResult {
    /// No worker finished the test.
    Missing,
    /// A worker skipped the test.
    Skipped,
    /// A worker ran the test.
    Finished(Box<TestMetadata>),
}

/// Reads the result of test `index` at `test_path`.
fn read_result(dir: &Path, index: usize, test_path: &TestPaths) -> io::Result<WorkerResult> {
    let bytes = match fs::read(dir.join(RESULTS_DIR).join(index.to_string())) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(WorkerResult::Missing),
        Err(e) => return Err(e),
    };
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let newline = bytes.iter().position(|&byte| byte == b'\n').ok_or_else(|| invalid("no header line"))?;
    let Some(header) = serde_json::from_slice::<Option<ResultHeader>>(&bytes[..newline])? else {
        return Ok(WorkerResult::Skipped);
    };
    if !matches!(header.status.as_str(), "passed" | "failed" | "errored" | "ignored passed" | "ignored failed") {
        return Err(invalid(&format!("unknown status {:?}", header.status)));
    }
    let (actual, rest) = bytes[newline + 1..].split_at_checked(header.actual_len).ok_or_else(|| invalid("short"))?;
    let (expected, diff) = rest.split_at_checked(header.expected_len).ok_or_else(|| invalid("short"))?;
    if diff.len() != header.diff_len {
        return Err(invalid("wrong length"));
    }
    Ok(WorkerResult::Finished(Box::new(TestMetadata {
        paths: test_path.clone(),
        id: header.id,
        status: header.status,
        color: header.color,
        ignored_test: header.ignored_test,
        render_test: header.render_test,
        outputs_image: header.outputs_image,
        size: Size::new(header.width, header.height),
        metrics_errored: header.metrics_errored,
        metrics_failed: header.metrics_failed,
        render_errored: header.render_errored,
        render_failed: header.render_failed,
        label_cut_off_found: header.label_cut_off_found,
        error_message: header.error_message,
        difference: f64::from_bits(header.difference),
        actual: actual.to_vec(),
        expected: expected.to_vec(),
        diff: diff.to_vec(),
        ..TestMetadata::default()
    })))
}
