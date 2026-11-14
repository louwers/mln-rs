//! `mln-bench`, the benchmarks' command (README.md):
//!
//! ```text
//! mln-bench run [--filter <s>]... [--scale <x>] [--fixtures <dir>] [--frames <dir>] [--json]
//! mln-bench compare <ref> [--rounds <n>] [--threshold <percent>] [--verbose]
//!                         [--fail-on <metric>]... [--filter <s>]... [--scale <x>]
//!                         [--fixtures <dir>]
//! mln-bench list
//! ```
//!
//! `--filter` keeps the cases whose name contains one of the strings;
//! `--scale` multiplies every case's iteration count; `--fail-on` makes
//! `compare` fail when the metric gets a worse verdict in any case.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mln_actor::RunLoop;
use mln_bench::meter::CountingAllocator;
use mln_bench::scene::{Mode, Scene};
use mln_bench::{CaseReport, Context, Environment, Report, cases, compare};
use mln_util::size::Size;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() -> ExitCode {
    // Before any thread starts, so that the counters count them all.
    mln_bench::meter::start_counters();
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mln-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The options all commands share, and the rest.
struct Options {
    filters: Vec<String>,
    scale: Option<String>,
    fixtures: PathBuf,
    frames: Option<PathBuf>,
    json: bool,
    rounds: usize,
    threshold: f64,
    /// `compare`'s metrics whose worse verdict fails it.
    fail_on: Vec<String>,
    verbose: bool,
    positional: Vec<String>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut options = Options {
        filters: Vec::new(),
        scale: None,
        fixtures: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bench-fixtures"),
        frames: None,
        json: false,
        rounds: 6,
        threshold: 5.0,
        fail_on: Vec::new(),
        verbose: false,
        positional: Vec::new(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--filter" => options.filters.push(value()?),
            "--scale" => options.scale = Some(value()?),
            "--fixtures" => options.fixtures = value()?.into(),
            "--frames" => options.frames = Some(value()?.into()),
            "--rounds" => options.rounds = value()?.parse().map_err(|_| "--rounds needs a number")?,
            "--threshold" => options.threshold = value()?.parse().map_err(|_| "--threshold needs a number")?,
            "--fail-on" => options.fail_on.push(value()?),
            "--json" => options.json = true,
            "--verbose" => options.verbose = true,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => options.positional.push(arg),
        }
    }
    Ok(options)
}

fn run(args: Vec<String>) -> Result<(), String> {
    let options = parse(args)?;
    let (command, rest) = options.positional.split_first().map_or(("run", &[][..]), |(c, r)| (c.as_str(), r));
    match (command, rest) {
        ("run", []) => run_cases(&options),
        ("list", []) => {
            for case in selected(&options) {
                println!("{}", case.name);
            }
            Ok(())
        }
        ("compare", [base]) => {
            fixture_version(&options.fixtures)?;
            let mut run_args = Vec::new();
            for filter in &options.filters {
                run_args.extend(["--filter".to_string(), filter.clone()]);
            }
            if let Some(scale) = &options.scale {
                run_args.extend(["--scale".to_string(), scale.clone()]);
            }
            let (report, worse) = compare::compare(&compare::Options {
                base: base.clone(),
                rounds: options.rounds,
                threshold: options.threshold,
                run_args,
                fixtures: options.fixtures.clone(),
                verbose: options.verbose,
            })?;
            print!("{report}");
            let failed: Vec<String> = worse
                .into_iter()
                .filter(|(_, metric)| options.fail_on.contains(metric))
                .map(|(case, metric)| format!("{case} {metric}"))
                .collect();
            if failed.is_empty() { Ok(()) } else { Err(format!("worse: {}", failed.join(", "))) }
        }
        _ => Err("usage: mln-bench [run | list | compare <ref>] [options] (see tools/bench/README.md)".into()),
    }
}

fn selected(options: &Options) -> Vec<cases::Case> {
    cases::all()
        .into_iter()
        .filter(|case| options.filters.is_empty() || options.filters.iter().any(|f| case.name.contains(f.as_str())))
        .collect()
}

fn fixture_version(fixtures: &Path) -> Result<String, String> {
    std::fs::read_to_string(fixtures.join("version")).map(|v| v.trim().to_string()).map_err(|_| {
        format!(
            "no basemap fixture in {}; prepare it with `mise run bench:fixtures` (tools/bench/fixtures.py <dir>)",
            fixtures.display()
        )
    })
}

fn run_cases(options: &Options) -> Result<(), String> {
    let fixtures = fixture_version(&options.fixtures)?;
    let cx = Context {
        fixtures: options.fixtures.canonicalize().map_err(|e| format!("{}: {e}", options.fixtures.display()))?,
        scale: options.scale.as_deref().map_or(Ok(1.0), str::parse).map_err(|_| "--scale needs a number")?,
        frames: options.frames.clone(),
    };
    let _run_loop = RunLoop::new();
    let (gpu, software_gpu) = Scene::new(Size::new(1, 1), Mode::Still).gpu();
    let environment =
        Environment { gpu, software_gpu, os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH), fixtures };
    if software_gpu {
        eprintln!("mln-bench: {} is a software rasterizer, which measures something else than a GPU", environment.gpu);
    }
    let mut reports = Vec::new();
    for case in selected(options) {
        eprintln!("mln-bench: {}", case.name);
        let metrics = case.run(&cx);
        if !options.json {
            let values: Vec<String> = metrics.0.iter().map(|m| format!("{} {:.3}", m.name, m.value)).collect();
            println!("{:<24} {}", case.name, values.join("  "));
        }
        reports.push(CaseReport { name: case.name, metrics });
    }
    if options.json {
        let report = Report { environment, cases: reports };
        println!("{}", serde_json::to_string(&report).map_err(|e| e.to_string())?);
    }
    Ok(())
}
