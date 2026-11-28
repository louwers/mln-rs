//! `mln-bench`, the benchmarks of mln-rs (README.md).

mod aslr;
mod compare;
mod environment;
mod fixtures;
mod host;
mod meter;
mod report;
mod runner;
mod scenario;
mod scenarios;
mod stats;
mod support;

use std::path::PathBuf;
use std::process::ExitCode;

use meter::CountingAllocator;

#[cfg(feature = "layout-probe")]
mod layout_probe {
    include!(concat!(env!("OUT_DIR"), "/layout_probe.rs"));
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const USAGE: &str = "usage:
  mln-bench list [--filter <s>]... [--json]
  mln-bench run [--filter <s>]... [--images <dir>] [--json]
  mln-bench compare <ref> [--filter <s>]... [--rounds <n>] [--max-load <n>] [--own-harness] [--json]
  mln-bench fixtures [--from <dir>]

--filter keeps the scenarios whose names contain one of the strings.
run and compare run each scenario in a process of its own (mln-bench exec).
--images writes the last image each scenario drew to <dir>, as PNG files
named after the scenarios.
compare builds this checkout's harness against <ref> and compares the two
(tools/bench/README.md): each metric's change, and its noise, how far it
moves between runs, and for instructions between builds of the same code
laid out differently; a change over twice its noise is marked. --rounds
runs each side n times, and marks the wall time when a rank test finds it
changed. --own-harness builds the ref with its own harness instead, and
compares the scenarios both harnesses have.
Rounds run alone among the machine's benchmark runs,
and not at all while its load average is over --max-load (a quarter of its
cores by default).
fixtures installs the snapshot that tools/bench/fixtures.json names, from its
base URL or from a directory laid out as it is.";

/// Prints the warnings and errors of the libraries to stderr.
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

/// Makes the process's `log` logger print warnings and errors to stderr.
fn log_to_stderr() {
    if log::set_logger(&StderrLog).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

fn main() -> ExitCode {
    // Kept in the binary, never called.
    #[cfg(feature = "layout-probe")]
    {
        let probe: fn(u64) -> u64 = layout_probe::layout_probe;
        std::hint::black_box(probe);
    }
    if std::env::args().nth(1).as_deref() == Some("exec") {
        aslr::fix();
    }
    log_to_stderr();
    // Before any thread starts, so that the counters count them all.
    meter::start_counters();
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mln-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    filters: Vec<String>,
    json: bool,
    rounds: Option<usize>,
    max_load: Option<f64>,
    own_harness: bool,
    from: Option<PathBuf>,
    images: Option<PathBuf>,
    /// exec's: the file the scenario writes its last image to.
    image: Option<PathBuf>,
    positional: Vec<String>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut options = Options {
        filters: Vec::new(),
        json: false,
        rounds: None,
        max_load: None,
        own_harness: false,
        from: None,
        images: None,
        image: None,
        positional: Vec::new(),
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        let number = |value: String| value.parse::<f64>().map_err(|_| format!("{arg} needs a number"));
        match arg.as_str() {
            "--filter" => options.filters.push(value()?),
            "--json" => options.json = true,
            "--rounds" => options.rounds = Some(value()?.parse().map_err(|_| "--rounds needs a number")?),
            "--max-load" => options.max_load = Some(number(value()?)?),
            "--own-harness" => options.own_harness = true,
            "--from" => options.from = Some(PathBuf::from(value()?)),
            "--images" => options.images = Some(PathBuf::from(value()?)),
            "--image" => options.image = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}\n\n{USAGE}")),
            _ => options.positional.push(arg),
        }
    }
    Ok(options)
}

fn run(args: Vec<String>) -> Result<(), String> {
    let options = parse(args)?;
    let positional: Vec<&str> = options.positional.iter().map(String::as_str).collect();
    match positional.as_slice() {
        ["list"] => {
            let selected = scenarios::select(&options.filters);
            if options.json {
                let list: Vec<serde_json::Value> = selected
                    .iter()
                    .map(|scenario| {
                        serde_json::json!({
                            "name": scenario.name,
                            "kind": scenario.kind,
                            "question": scenario.question,
                        })
                    })
                    .collect();
                println!("{}", serde_json::Value::Array(list));
            } else {
                for scenario in selected {
                    println!("{}\n  {}", scenario.name, scenario.question);
                }
            }
            Ok(())
        }
        ["run"] => {
            let selected = scenarios::select(&options.filters);
            if selected.is_empty() {
                return Err("no scenario matches the filters".to_owned());
            }
            if cfg!(debug_assertions) {
                eprintln!("mln-bench: this build isn't optimized; `mise run bench` builds the benchmark profile");
            }
            runner::check_build()?;
            let binary = std::env::current_exe().map_err(|error| error.to_string())?;
            if let Some(dir) = &options.images {
                std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            }
            let _measuring = host::measuring(false)?;
            runner::warm_up(&binary, &selected)?;
            let mut runs = Vec::new();
            for scenario in selected {
                eprintln!("mln-bench: {}", scenario.name);
                let image =
                    options.images.as_ref().map(|dir| dir.join(format!("{}.png", scenario.name.replace('/', "-"))));
                runs.push(runner::run_in_process(&binary, scenario.name, image.as_deref())?);
            }
            if options.json {
                println!("{}", serde_json::to_string_pretty(&runs).map_err(|error| error.to_string())?);
            } else {
                print!("{}", report::runs_text(&runs));
            }
            Ok(())
        }
        ["compare", base] => {
            let comparison = compare::compare(&compare::Options {
                base: (*base).to_owned(),
                filters: options.filters,
                rounds: options.rounds,
                max_load: options.max_load.unwrap_or_else(host::default_max_load),
                own_harness: options.own_harness,
            })?;
            if options.json {
                println!("{}", serde_json::to_string_pretty(&comparison).map_err(|error| error.to_string())?);
            } else {
                print!("{}", compare::text(&comparison));
            }
            Ok(())
        }
        ["fixtures"] => {
            let origin = match &options.from {
                Some(dir) => fixtures::Origin::Dir(dir),
                None => fixtures::Origin::Url,
            };
            let manifest = fixtures::Manifest::current();
            let dir = fixtures::install(manifest, &fixtures::cache_root()?, &origin)?;
            // stderr, so that the JSON of a run that installs the fixtures
            // first stays alone on stdout.
            eprintln!("mln-bench: snapshot {} is installed in {}", manifest.snapshot, dir.display());
            Ok(())
        }
        ["exec", name, out] => {
            let scenario = scenarios::find(name).ok_or_else(|| format!("no scenario {name}"))?;
            runner::exec(scenario, &PathBuf::from(out), options.image)
        }
        _ => Err(USAGE.to_owned()),
    }
}
