//! `c-abi-test <suite> [--exit-programs <dir>] [--jobs <n>]`: runs
//! the C ABI suite against `crates/mln_c` (`mise run test:c-abi`).
//!
//! The suite binary is the one `CMakeLists.txt` builds. Each test file runs in
//! a process of its own, in parallel. A case that fails leaves its handles to the suite's `tearDown`, whose
//! callbacks can still reach the failed case's stack, so the file's later cases
//! each run in a process of their own. When a process dies (a crash, or the
//! suite's hang watchdog), the case it was running fails.
//!
//! With `--exit-programs`, it then runs each exit program that
//! `CMakeLists.txt` builds into that directory, in parallel. Each returns from
//! main while the library's threads are at work, so it passes when the process
//! exit that follows ends it with status 0 within `EXIT_TIMEOUT`.
//!
//! The run fails when any case fails or any exit program doesn't pass.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use regex::Regex;
use wait_timeout::ChildExt as _;

const USAGE: &str = "usage: c-abi-test <suite> [--exit-programs <dir>] [--jobs <n>]

<suite> is the suite binary, mln_native_abi_tests. --exit-programs names the
directory of the exit programs to run, and --jobs how many test files or exit
programs to run at once (0, the default, for one per CPU).";

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tests/native/fixtures");
/// The directory of the suite's test files, which its listing and results
/// name them under.
const PREFIX: &str = "tests/native/";
/// Unity's result line: `<file>:<line>:<case>:<PASS|FAIL|IGNORE>[:<message>]`.
static RESULT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^tests/native/(?P<file>[^:]+):\d+:(?P<case>\w+):(?P<outcome>PASS|FAIL|IGNORE)(?::(?P<message>.*))?")
        .unwrap()
});
/// A process gets at least the watchdog's 30 s for each of its cases.
const PROCESS_TIMEOUT: Duration = Duration::from_mins(10);
const EXIT_TIMEOUT: Duration = Duration::from_mins(1);

struct Args {
    suite: PathBuf,
    exit_programs: Option<PathBuf>,
    jobs: usize,
}

enum Outcome {
    Pass,
    Fail(String),
    Ignore,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("c-abi-test: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The arguments, or `None` for `--help`.
fn parse_args() -> Result<Option<Args>, String> {
    let mut suite = None;
    let mut exit_programs = None;
    let mut jobs = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, mut inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut next_value = || inline.take().or_else(|| args.next()).ok_or(USAGE);
        match flag {
            "--exit-programs" => exit_programs = Some(next_value()?.into()),
            "--jobs" => jobs = next_value()?.parse().map_err(|_| USAGE)?,
            "-h" | "--help" => return Ok(None),
            _ if suite.is_none() && !flag.starts_with('-') => suite = Some(arg.into()),
            _ => return Err(USAGE.into()),
        }
    }
    if jobs == 0 {
        jobs = std::thread::available_parallelism().map_or(1, usize::from);
    }
    Ok(Some(Args { suite: suite.ok_or(USAGE)?, exit_programs, jobs }))
}

/// Whether every case and exit program passed.
fn run(args: &Args) -> Result<bool, String> {
    let suite = args.suite.canonicalize().map_err(|error| format!("{}: {error}", args.suite.display()))?;
    let files = list_cases(&suite)?;
    let mut results = BTreeMap::new();
    // The cases after a failure, which run on their own once every file has.
    let mut alone = Vec::new();
    for ran in parallel_map(&files, args.jobs, |(file, cases)| run_file(&suite, file, cases)) {
        let (mut outcomes, rest) = ran?;
        results.append(&mut outcomes);
        alone.extend(rest);
    }
    let outcomes = parallel_map(&alone, args.jobs, |key| run_case(&suite, key));
    for (key, outcome) in alone.into_iter().zip(outcomes) {
        results.insert(key, outcome?);
    }
    let failures: Vec<(&String, &String)> = results
        .iter()
        .filter_map(|(key, outcome)| match outcome {
            Outcome::Fail(message) => Some((key, message)),
            Outcome::Pass | Outcome::Ignore => None,
        })
        .collect();
    let ignored = results.values().filter(|outcome| matches!(outcome, Outcome::Ignore)).count();
    println!(
        "{} cases in {} files: {} passed, {} failed, {ignored} ignored",
        results.len(),
        files.len(),
        results.len() - failures.len() - ignored,
        failures.len(),
    );
    for (key, message) in &failures {
        println!("FAILED: {key}: {message}");
    }
    let mut passed = failures.is_empty();

    if let Some(directory) = &args.exit_programs {
        let programs = exit_programs(directory)?;
        let failures: Vec<(String, String)> = programs
            .iter()
            .zip(parallel_map(&programs, args.jobs, |program| run_exit_program(program)))
            .filter_map(|(program, result)| {
                result.err().map(|why| (format!("exit/{}.c", program.file_name().unwrap_or_default().display()), why))
            })
            .collect();
        println!(
            "{} exit programs: {} passed, {} failed",
            programs.len(),
            programs.len() - failures.len(),
            failures.len()
        );
        for (key, why) in &failures {
            println!("FAILED: {key}: {why}");
        }
        passed &= failures.is_empty();
    }
    Ok(passed)
}

/// The suite's test files, sorted, and each one's cases in the suite's order.
fn list_cases(suite: &Path) -> Result<Vec<(String, Vec<String>)>, String> {
    let output = command(suite).arg("-l").output().map_err(|error| format!("{}: {error}", suite.display()))?;
    if !output.status.success() {
        return Err(format!("{} -l {}", suite.display(), ending(output.status)));
    }
    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((file, case)) = line.split_once(':')
            && let Some(file) = file.strip_prefix(PREFIX)
            && !case.is_empty()
        {
            files.entry(file.to_owned()).or_default().push(case.to_owned());
        }
    }
    if files.is_empty() {
        return Err(format!("{} -l listed no cases", suite.display()));
    }
    Ok(files.into_iter().collect())
}

/// Runs the whole file: the outcomes of its cases up to the first that fails,
/// keyed `<file>:<case>`, and the keys of the cases after it, which must each
/// run on their own.
fn run_file(suite: &Path, file: &str, cases: &[String]) -> Result<(BTreeMap<String, Outcome>, Vec<String>), String> {
    let mut together = run_suite(suite, &["-f", &format!("{PREFIX}{file}")])?;
    let mut outcomes = BTreeMap::new();
    let mut keys = cases.iter().map(|case| format!("{file}:{case}"));
    for key in keys.by_ref() {
        let outcome = together.outcome(&key);
        let failed = matches!(outcome, Outcome::Fail(_));
        outcomes.insert(key, outcome);
        if failed {
            break;
        }
    }
    Ok((outcomes, keys.collect()))
}

/// The outcome of the case `<file>:<case>`, run on its own.
fn run_case(suite: &Path, key: &str) -> Result<Outcome, String> {
    Ok(run_suite(suite, &["-n", &format!("{PREFIX}{key}")])?.outcome(key))
}

/// The outcomes that a run of the suite reported.
struct Report {
    /// Keyed `<file>:<case>`.
    outcomes: BTreeMap<String, Outcome>,
    /// Why the run stopped, if it died.
    died: String,
}

impl Report {
    /// The case's outcome, or a failure if the run died before reporting it.
    fn outcome(&mut self, key: &str) -> Outcome {
        self.outcomes.remove(key).unwrap_or_else(|| Outcome::Fail(self.died.clone()))
    }
}

fn run_suite(suite: &Path, filter: &[&str]) -> Result<Report, String> {
    let mut command = command(suite);
    command.args(filter);
    let (output, status) = run_with_timeout(command, PROCESS_TIMEOUT)?;
    let mut outcomes = BTreeMap::new();
    for result in output.lines().filter_map(|line| RESULT.captures(line)) {
        let outcome = match &result["outcome"] {
            "PASS" => Outcome::Pass,
            "IGNORE" => Outcome::Ignore,
            _ => Outcome::Fail(result.name("message").map_or("", |message| message.as_str().trim()).to_owned()),
        };
        outcomes.insert(format!("{}:{}", &result["file"], &result["case"]), outcome);
    }
    let died = if output.lines().any(|line| line.starts_with("MLN_TEST_HANG")) {
        "hung (the suite's watchdog)".to_owned()
    } else {
        match status {
            Some(status) => format!("{} before reporting", ending(status)),
            None => format!("timed out after {} s", PROCESS_TIMEOUT.as_secs()),
        }
    };
    Ok(Report { outcomes, died })
}

/// The executables in the directory, sorted.
fn exit_programs(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let error = |error: std::io::Error| format!("{}: {error}", directory.display());
    let mut programs = Vec::new();
    for entry in directory.read_dir().map_err(error)? {
        let path = entry.map_err(error)?.path();
        if is_executable(&path) {
            programs.push(path);
        }
    }
    programs.sort();
    if programs.is_empty() {
        return Err(format!("{} has no exit programs", directory.display()));
    }
    Ok(programs)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata().is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Runs the exit program, and fails with why it didn't pass.
fn run_exit_program(program: &Path) -> Result<(), String> {
    let (output, status) = run_with_timeout(command(program), EXIT_TIMEOUT)?;
    let status = status.ok_or_else(|| format!("didn't exit within {} s", EXIT_TIMEOUT.as_secs()))?;
    if status.success() {
        return Ok(());
    }
    Err(match output.trim() {
        "" => ending(status),
        output => format!("{}: {}", ending(status), output.replace('\n', "; ")),
    })
}

/// The program, with the suite's fixtures.
fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.env("MLN_FFI_TEST_FIXTURE_DIR", FIXTURES);
    command
}

/// What the command wrote to stdout and then stderr, and how it exited, or
/// `None` if it ran past the timeout and was killed.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(String, Option<ExitStatus>), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let error = |error: std::io::Error| format!("{program}: {error}");
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(error)?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    std::thread::scope(|scope| {
        let stdout = scope.spawn(|| read_all(stdout));
        let stderr = scope.spawn(|| read_all(stderr));
        let status = child.wait_timeout(timeout).map_err(error)?;
        if status.is_none() {
            // Gone already if it exited since the wait timed out.
            let _ = child.kill();
            child.wait().map_err(error)?;
        }
        let mut output = stdout.join().unwrap_or_default();
        output.extend(stderr.join().unwrap_or_default());
        Ok((String::from_utf8_lossy(&output).into_owned(), status))
    })
}

fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        // What it read before an error is all there is to report.
        let _ = pipe.read_to_end(&mut bytes);
    }
    bytes
}

/// How a process ended: "exited with status 1" or "died of signal 11".
fn ending(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exited with status {code}");
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return format!("died of signal {signal}");
    }
    status.to_string()
}

/// `f` of each item, in the items' order, on up to `jobs` threads.
fn parallel_map<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs.min(items.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else { break done };
                        done.push((index, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}
