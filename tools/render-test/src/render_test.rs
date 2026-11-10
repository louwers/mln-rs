// Ports render-test/render_test.cpp (`mln::runRenderTests`).
//
// Options that C++ does not have:
// - `--output <dir>` (required): where actual.png, diff.png and the HTML
//   report go. C++ writes them next to the tests and the manifest; this
//   runner never writes into the C++ tree.
// - `--self-check`: renders nothing; every test's "actual" image is its
//   expected image, so the run exercises manifest parsing, metadata and
//   operation parsing, comparison and reporting. Exits non-zero unless
//   every test that runs passes (ignored ones included).
// - `--timeout=<seconds>`: the per-test time limit (watchdog.rs).
// - `--jobs=<n>`: runs the tests in n worker processes (jobs.rs), which run
//   each test with the same `run_test` as the loop here.
// - `--tileWorkerCodec=<n>`: puts the maps' tile workers in n worker
//   instances that the map reaches only through bytes (mln_renderer's
//   worker_codec, as the browser build does), so the suites run through the
//   codec.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::filesystem;
use crate::jobs;
use crate::log::{self, Event};
use crate::manifest_parser::{Manifest, ManifestParser};
use crate::metadata::{TestMetadata, TestPaths, TestStatistics};
use crate::parser::{create_result_page, parse_test_metadata, serialize_json_value};
use crate::runner::{ImplFactory, ImplOptions, TestRunner, UpdateResults};

// SHOW_ANSI_COLORS is not defined in the C++ build either.
const ANSI_COLOR_RED: &str = "";
const ANSI_COLOR_GREEN: &str = "";
const ANSI_COLOR_YELLOW: &str = "";
const ANSI_COLOR_GRAY: &str = "";
const ANSI_COLOR_LIGHT_GRAY: &str = "";
const ANSI_COLOR_RESET: &str = "";

const HELP: &str = "  render-test {OPTIONS}

    MapLibre Native Test Runner

  OPTIONS:

      -h, --help                        Display this help menu
      -r, --recycle-map                 Toggle reusing the map object
      -s, --shuffle                     Toggle shuffling the tests order
      -o, --online                      Toggle online mode (by default tests
                                        will run offline)
      --seed=[seed]                     Shuffle seed (default: random)
      -p[manifestPath],
      --manifestPath=[manifestPath]     Test manifest file path
      -f[filter], --filter=[filter]     Test filter regex
      -u[update], --update=[update]     Test results update mode. Supported
                                        values are: \"default\" Updates generic
                                        render test expectations. \"platform\"
                                        Updates platform specific render test
                                        expectations. \"metrics\" Updates
                                        expected metrics for configuration
                                        defined by a manifest. \"rebaseline\"
                                        Updates or creates expected metrics for
                                        configuration defined by a manifest.
      --output=[dir]                    Directory for actual.png, diff.png
                                        and the HTML report
      --self-check                      Compare each test's expected image
                                        with itself instead of rendering
      --timeout=[seconds]               Error a test that runs longer than
                                        this (default 60, 0: no limit)
      --jobs=[n]                        Run the tests in n worker processes
                                        (default 1: in this process; 0: one
                                        per CPU)
      --tileWorkerCodec=[n]             Run the tile workers in n worker
                                        instances behind the worker codec
                                        (default 0: on the map's thread pool)
";

/// `ArgumentsTuple` plus the options C++ does not have. Serialized for the
/// workers of `--jobs` (jobs.rs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arguments {
    pub recycle_map: bool,
    pub shuffle: bool,
    pub online: bool,
    pub seed: u32,
    pub manifest_path: String,
    pub update_results: UpdateResults,
    pub test_filter: String,
    pub self_check: bool,
    pub output: Option<String>,
    /// The per-test wall-clock limit in milliseconds (0: none).
    pub timeout_millis: u64,
    /// The number of worker processes that run the tests (1: the tests run
    /// in this process; 0: one per CPU). See jobs.rs.
    pub jobs: usize,
    /// The number of worker instances behind the worker codec that run the
    /// maps' tile workers (0: the map's thread pool). See `ImplOptions`.
    pub tile_worker_codec: usize,
    /// Not an option: the `render-test` executable the workers
    /// run (`None`: this executable). Tests, which run the runner from the
    /// test harness, set the package's binary.
    pub worker_program: Option<PathBuf>,
}

/// `parseArguments(argc, argv)` (args.hxx). C++ calls `exit(code)`; here
/// `Err(code)` is returned after the same messages.
pub fn parse_arguments(args: &[String]) -> Result<Arguments, i32> {
    let mut recycle_map = false;
    let mut shuffle = false;
    let mut online = false;
    let mut seed: Option<u32> = None;
    let mut manifest_path: Option<String> = None;
    let mut test_filter: Option<String> = None;
    let mut update: Option<String> = None;
    let mut self_check = false;
    let mut output: Option<String> = None;
    let mut timeout_millis: u64 = 60_000;
    let mut jobs: usize = 1;
    let mut tile_worker_codec: usize = 0;

    let parse_error = |message: String| -> i32 {
        log::info(Event::General, HELP);
        log::error(Event::General, &message);
        1
    };

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        // Long options: --name or --name=value.
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline_value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let mut take_value = |what: &str| -> Result<String, i32> {
                if let Some(v) = inline_value.clone() {
                    return Ok(v);
                }
                if i < args.len() {
                    i += 1;
                    return Ok(args[i - 1].clone());
                }
                Err(parse_error(format!("Flag '{what}' requires an argument but received none")))
            };
            match name {
                "help" => {
                    log::info(Event::General, HELP);
                    return Err(0);
                }
                "recycle-map" => recycle_map = true,
                "shuffle" => shuffle = true,
                "online" => online = true,
                "self-check" => self_check = true,
                "seed" => {
                    let value = take_value("--seed")?;
                    seed =
                        Some(value.parse().map_err(|_| {
                            parse_error(format!("Argument 'seed' received invalid value type '{value}'"))
                        })?);
                }
                "manifestPath" => manifest_path = Some(take_value("--manifestPath")?),
                "filter" => test_filter = Some(take_value("--filter")?),
                "update" => update = Some(take_value("--update")?),
                "output" => output = Some(take_value("--output")?),
                "timeout" => {
                    let value = take_value("--timeout")?;
                    timeout_millis = value
                        .parse::<f64>()
                        .ok()
                        .filter(|t| t.is_finite() && *t >= 0.0)
                        .map(|t| (t * 1000.0) as u64)
                        .ok_or_else(|| {
                            parse_error(format!("Argument 'timeout' received invalid value type '{value}'"))
                        })?;
                }
                "jobs" => {
                    let value = take_value("--jobs")?;
                    jobs = value
                        .parse()
                        .map_err(|_| parse_error(format!("Argument 'jobs' received invalid value type '{value}'")))?;
                }
                "tileWorkerCodec" => {
                    let value = take_value("--tileWorkerCodec")?;
                    tile_worker_codec = value.parse().map_err(|_| {
                        parse_error(format!("Argument 'tileWorkerCodec' received invalid value type '{value}'"))
                    })?;
                }
                _ => return Err(parse_error(format!("Flag could not be matched: {name}"))),
            }
            continue;
        }
        // Short options: -x, -xvalue, -x value, grouped flags (-rs).
        if let Some(short) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let chars: Vec<char> = short.chars().collect();
            let mut j = 0;
            while j < chars.len() {
                let c = chars[j];
                j += 1;
                match c {
                    'h' => {
                        log::info(Event::General, HELP);
                        return Err(0);
                    }
                    'r' => recycle_map = true,
                    's' => shuffle = true,
                    'o' => online = true,
                    'p' | 'f' | 'u' => {
                        let value = if j < chars.len() {
                            let rest: String = chars[j..].iter().collect();
                            j = chars.len();
                            rest
                        } else if i < args.len() {
                            i += 1;
                            args[i - 1].clone()
                        } else {
                            return Err(parse_error(format!("Flag '{c}' requires an argument but received none")));
                        };
                        match c {
                            'p' => manifest_path = Some(value),
                            'f' => test_filter = Some(value),
                            _ => update = Some(value),
                        }
                    }
                    _ => return Err(parse_error(format!("Flag could not be matched: {c}"))),
                }
            }
            continue;
        }
        return Err(parse_error(format!(
            "Passed in argument, but no positional arguments were ready to receive it: {arg}"
        )));
    }

    let update_results = match update.as_deref() {
        None => UpdateResults::No,
        Some("default") => UpdateResults::Default,
        Some("platform") => UpdateResults::Platform,
        Some("metrics") => UpdateResults::Metrics,
        Some("rebaseline") => UpdateResults::Rebaseline,
        Some(other) => return Err(parse_error(format!("Could not find key '{other}' in map for arg 'update'"))),
    };

    let Some(manifest_path) = manifest_path else {
        log::info(Event::General, HELP);
        log::error(Event::General, "Option 'manifestPath' is required");
        return Err(2);
    };

    if !filesystem::exists(&manifest_path) || filesystem::filename(&manifest_path).is_empty() {
        log::error(Event::General, &format!("Provided test manifest file path '{manifest_path}' does not exist"));
        return Err(3);
    }

    // The port's restrictions.
    if update_results != UpdateResults::No {
        log::error(
            Event::General,
            "TODO(port): --update is not supported: the port never writes baselines into \
             ../maplibre-native (add an alternate expectation by hand for a justified difference)",
        );
        return Err(1);
    }

    Ok(Arguments {
        recycle_map,
        shuffle,
        online,
        seed: seed.unwrap_or(1),
        manifest_path,
        update_results,
        test_filter: test_filter.unwrap_or_default(),
        self_check,
        output,
        timeout_millis,
        jobs,
        tile_worker_codec,
        worker_program: None,
    })
}

/// `runWithAlternateSources(runner, metadata, errored, passed)`.
fn run_with_alternate_sources(
    runner: &mut TestRunner,
    metadata: &mut TestMetadata,
    errored: &mut bool,
    passed: &mut bool,
) {
    const REPLACEMENTS_KEY: &str = "source-replace";
    const TARGET_KEY: &str = "replace";
    const SOURCE_KEY: &str = "with";
    const SOURCES_KEY: &str = "sources";

    let Some(test_element) = metadata.test_value() else {
        return;
    };
    let Some(replaces_element) = test_element.get(REPLACEMENTS_KEY).and_then(|r| r.as_array()) else {
        return;
    };
    if replaces_element.is_empty() {
        return;
    }
    let replaces_element = replaces_element.clone();

    // Each item in the source replacements array...
    for entry in &replaces_element {
        let (Some(target_name), Some(source_name)) =
            (entry.get(TARGET_KEY).and_then(|v| v.as_str()), entry.get(SOURCE_KEY).and_then(|v| v.as_str()))
        else {
            continue;
        };
        let swappable = |document: &serde_json::Value| {
            let sources = &document[SOURCES_KEY];
            sources.get(target_name).is_some_and(|v| v.is_object())
                && sources.get(source_name).is_some_and(|v| v.is_object())
        };
        if !swappable(&metadata.document) {
            continue;
        }

        // `Value::Swap`: the two members keep their positions.
        let swap = |document: &mut serde_json::Value| {
            let sources = &mut document[SOURCES_KEY];
            let target = sources[target_name].take();
            let source = std::mem::replace(&mut sources[source_name], target);
            sources[target_name] = source;
        };
        let original_json = std::mem::take(&mut metadata.document_json);

        // Swap the named sources
        swap(&mut metadata.document);
        metadata.document_json = serialize_json_value(&metadata.document);

        // run the test again with the new source
        runner.run(metadata);

        // Restore the original source
        swap(&mut metadata.document);
        metadata.document_json = original_json;

        // stop if the test fails
        *errored = metadata.metrics_errored != 0 || metadata.render_errored != 0 || metadata.label_cut_off_found != 0;
        *passed = !*errored && metadata.metrics_failed == 0 && metadata.render_failed == 0;
        if !*passed {
            break;
        }
    }
}

/// The `TestRunner` of `runRenderTests`, with the per-test time limit.
pub(crate) fn create_runner(
    arguments: &Arguments,
    manifest: Manifest,
    factory: Option<ImplFactory>,
    output_path: PathBuf,
) -> TestRunner {
    let mut runner = TestRunner::new(manifest, arguments.update_results, factory, arguments.self_check, output_path);
    runner.set_impl_options(ImplOptions { tile_worker_codec: arguments.tile_worker_codec });
    if !arguments.self_check && arguments.timeout_millis > 0 {
        runner.set_test_timeout(Some(std::time::Duration::from_millis(arguments.timeout_millis)));
    }
    runner
}

/// One iteration of the `runRenderTests` loop: runs the test at `test_path`,
/// sets its status and prints it. `None`: the test is skipped.
pub(crate) fn run_test(
    runner: &mut TestRunner,
    test_path: &TestPaths,
    ignores: &[(String, String)],
    root_length: usize,
    recycle_map: bool,
) -> Option<TestMetadata> {
    if !recycle_map {
        runner.reset();
    }

    let (mut metadata, ignore_reason) = prepare_test(test_path, ignores, root_length)?;

    if metadata.document_object_empty() {
        metadata.metrics_errored += 1;
        metadata.render_errored += 1;
    } else {
        runner.run(&mut metadata);
    }

    let mut errored =
        metadata.metrics_errored != 0 || metadata.render_errored != 0 || metadata.label_cut_off_found != 0;
    let mut passed = !errored && metadata.metrics_failed == 0 && metadata.render_failed == 0;

    if passed {
        run_with_alternate_sources(runner, &mut metadata, &mut errored, &mut passed);
    }

    finish_test(&mut metadata, ignore_reason.as_deref(), errored, passed);
    Some(metadata)
}

/// The start of the `runRenderTests` loop body: the test's metadata, with
/// its id, and its ignore reason if the ignores list it. `None` (after
/// printing so) when the test is skipped: its ignore reason starts with
/// "skip".
pub(crate) fn prepare_test(
    test_path: &TestPaths,
    ignores: &[(String, String)],
    root_length: usize,
) -> Option<(TestMetadata, Option<String>)> {
    let mut metadata = parse_test_metadata(test_path);

    let default_expectations = test_path.default_expectations();
    metadata.id = default_expectations[root_length + 1..default_expectations.len() - 1].to_string();
    let id = metadata.id.clone();

    // `pair.first == ignoreName` compares std::filesystem paths.
    let ignore_name = Path::new(&id);
    let mut ignore_reason = None;
    if let Some((_, reason)) = ignores.iter().find(|(name, _)| Path::new(name) == ignore_name) {
        metadata.ignored_test = true;
        if reason.starts_with("skip") {
            println!("{ANSI_COLOR_GRAY}* skipped {id} ({reason}){ANSI_COLOR_RESET}");
            log::info(Event::General, &format!("* skipped {id}({reason})"));
            return None;
        }
        ignore_reason = Some(reason.clone());
    }
    Some((metadata, ignore_reason))
}

/// The end of the `runRenderTests` loop body: the status and color of a
/// test that ran, and its printed result. The statistics and the return
/// code follow from the status (`tally`).
pub(crate) fn finish_test(metadata: &mut TestMetadata, ignore_reason: Option<&str>, errored: bool, passed: bool) {
    let id = metadata.id.clone();
    if let Some(ignore_reason) = ignore_reason {
        if passed {
            metadata.status = "ignored passed".to_string();
            metadata.color = "#E8A408".to_string();
            println!("{ANSI_COLOR_YELLOW}* ignore {id} ({ignore_reason}){ANSI_COLOR_RESET}");
            log::info(Event::General, &format!("* ignore {id} ({ignore_reason})"));
        } else {
            metadata.status = "ignored failed".to_string();
            metadata.color = "#9E9E9E".to_string();
            println!("{ANSI_COLOR_LIGHT_GRAY}* ignore {id} ({ignore_reason}){ANSI_COLOR_RESET}");
            log::info(Event::General, &format!("* ignore {id} ({ignore_reason})"));
        }
    } else if passed {
        metadata.status = "passed".to_string();
        metadata.color = "green".to_string();
        println!("{ANSI_COLOR_GREEN}* passed {id}{ANSI_COLOR_RESET}");
        log::info(Event::General, &format!("* passed {id}"));
    } else if errored {
        metadata.status = "errored".to_string();
        metadata.color = "red".to_string();
        println!("{ANSI_COLOR_RED}* errored {id}{ANSI_COLOR_RESET}");
        println!("{ANSI_COLOR_RED}* error: {}{ANSI_COLOR_RESET}", metadata.error_message);
        log::info(Event::General, &format!("* errored {id}"));
        log::info(Event::General, &format!("* error {}", metadata.error_message));
    } else {
        metadata.status = "failed".to_string();
        metadata.color = "red".to_string();
        println!("{ANSI_COLOR_RED}* failed {id}{ANSI_COLOR_RESET}");
        log::info(Event::General, &format!("* failed {id}"));
    }
}

/// The statistics and the return code of the tests that ran, in run order:
/// an errored test that is not ignored sets 2, a failed one 3, and the last
/// one wins.
fn tally(metadatas: &[TestMetadata]) -> (TestStatistics, i32) {
    let mut stats = TestStatistics::default();
    let mut return_code = 0;
    for metadata in metadatas {
        match metadata.status.as_str() {
            "ignored passed" => stats.ignore_passed_tests += 1,
            "ignored failed" => stats.ignore_failed_tests += 1,
            "passed" => stats.passed_tests += 1,
            "errored" => {
                stats.errored_tests += 1;
                return_code = 2;
            }
            "failed" => {
                stats.failed_tests += 1;
                return_code = 3;
            }
            status => unreachable!("finish_test sets no status {status:?}"),
        }
    }
    (stats, return_code)
}

/// What a run produced: the process exit code, the statistics and the
/// metadata of every test that was not skipped, in run order. (With
/// `--jobs`, the metadata holds what the report shows: see jobs.rs.)
#[derive(Debug)]
pub struct RunResult {
    pub return_code: i32,
    pub stats: TestStatistics,
    pub metadatas: Vec<TestMetadata>,
}

/// `mln::runRenderTests(argc, argv, testStatus)`. `factory` creates the map
/// for a test (`None`: only `--self-check` can pass).
pub fn run_render_tests(args: &[String], factory: Option<ImplFactory>) -> i32 {
    // A worker of a `--jobs` run (jobs.rs).
    if let [_, flag, dir] = args
        && flag == jobs::WORKER_FLAG
    {
        return jobs::run_worker(Path::new(dir), factory);
    }
    match parse_arguments(args) {
        Ok(arguments) => run_render_tests_with(&arguments, factory).return_code,
        Err(code) => code,
    }
}

/// `runRenderTests` after argument parsing.
pub fn run_render_tests_with(arguments: &Arguments, factory: Option<ImplFactory>) -> RunResult {
    let early = |return_code: i32| RunResult { return_code, stats: TestStatistics::default(), metadatas: Vec::new() };

    let Arguments { recycle_map, shuffle, online, seed, manifest_path, test_filter, .. } = arguments.clone();

    crate::file_source::ProxyFileSource::set_offline(!online);
    // TODO(port): NetworkStatus::Set(online ? Online : Offline) (online file
    // source).

    let Some(mut manifest_data) = ManifestParser::parse_manifest(&manifest_path, test_filter) else {
        return early(5);
    };

    let Some(output_path) = arguments.output.as_deref().map(PathBuf::from) else {
        log::error(Event::General, "No output directory: pass --output <dir>");
        return early(1);
    };

    if shuffle {
        // printf("%d", uint32_t)
        println!("{ANSI_COLOR_YELLOW}Shuffle seed: {}{ANSI_COLOR_RESET}", seed as i32);
        // The manifest, not the runner: a `--jobs` run has no runner here.
        manifest_data.do_shuffle(seed);
    }

    let jobs = jobs::job_count(arguments.jobs, manifest_data.get_test_paths().len());
    let metadatas = if jobs > 1 {
        jobs::run_jobs(arguments, &manifest_data, &output_path, jobs)
    } else {
        let ignores = manifest_data.get_ignores().to_vec();
        let test_paths = manifest_data.get_test_paths().to_vec();
        let root_length = manifest_data.get_test_root_path().len();
        // mln::util::RunLoop runLoop: created by the map factory (map::factory).
        let mut runner = create_runner(arguments, manifest_data, factory, output_path.clone());
        test_paths
            .iter()
            .filter_map(|test_path| run_test(&mut runner, test_path, &ignores, root_length, recycle_map))
            .collect()
    };

    let (stats, mut return_code) = tally(&metadatas);

    let manifest_name = filesystem::stem(&manifest_path).to_string();
    let result_path = output_path.join(format!("{manifest_name}.html"));
    let results_html = create_result_page(&stats, &metadatas, shuffle, seed);
    if let Err(e) = std::fs::create_dir_all(&output_path).and_then(|()| std::fs::write(&result_path, results_html)) {
        log::error(Event::General, &format!("Failed to write {}: {e}", result_path.display()));
    }

    let count = stats.errored_tests
        + stats.failed_tests
        + stats.ignore_failed_tests
        + stats.ignore_passed_tests
        + stats.passed_tests;
    let percent = |n: u32| 100.0 * f64::from(n) / f64::from(count);

    if stats.passed_tests != 0 {
        println!(
            "{ANSI_COLOR_GREEN}{} passed ({:.1}%){ANSI_COLOR_RESET}",
            stats.passed_tests,
            percent(stats.passed_tests)
        );
        log::info(Event::General, &format!("{} passed tests", stats.passed_tests));
    }
    if stats.ignore_passed_tests != 0 {
        println!(
            "{ANSI_COLOR_YELLOW}{} passed but were ignored ({:.1}%){ANSI_COLOR_RESET}",
            stats.ignore_passed_tests,
            percent(stats.ignore_passed_tests)
        );
        log::info(Event::General, &format!("{} passed tests but were ignored", stats.ignore_passed_tests));
    }
    if stats.ignore_failed_tests != 0 {
        println!(
            "{ANSI_COLOR_LIGHT_GRAY}{} ignored ({:.1}%){ANSI_COLOR_RESET}",
            stats.ignore_failed_tests,
            percent(stats.ignore_failed_tests)
        );
        log::info(Event::General, &format!("{} ignored tests", stats.ignore_failed_tests));
    }
    if stats.failed_tests != 0 {
        println!(
            "{ANSI_COLOR_RED}{} failed ({:.1}%){ANSI_COLOR_RESET}",
            stats.failed_tests,
            percent(stats.failed_tests)
        );
        log::info(Event::General, &format!("{} failed tests", stats.failed_tests));
    }
    if stats.errored_tests != 0 {
        println!(
            "{ANSI_COLOR_RED}{} errored ({:.1}%){ANSI_COLOR_RESET}",
            stats.errored_tests,
            percent(stats.errored_tests)
        );
        log::info(Event::General, &format!("{} errored tests", stats.errored_tests));
    }

    let canonical = std::fs::canonicalize(&result_path).unwrap_or(result_path);
    println!("Results at: {}", canonical.display());

    if arguments.self_check {
        let not_passed = stats.failed_tests + stats.errored_tests + stats.ignore_failed_tests;
        if count == 0 || not_passed != 0 {
            println!("self-check: {not_passed} of {count} tests did not pass");
            if return_code == 0 {
                return_code = 4;
            }
        } else {
            println!("self-check: all {count} tests passed");
        }
    }

    RunResult { return_code, stats, metadatas }
}
