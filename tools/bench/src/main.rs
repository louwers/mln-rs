//! `mln-bench`, the benchmarks of mln-rs (README.md).

mod compare;
mod environment;
mod fixtures;
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

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const USAGE: &str = "usage:
  mln-bench list [--filter <s>]... [--json]
  mln-bench run [--filter <s>]... [--json]
  mln-bench compare <ref> [--filter <s>]... [--rounds <n>] [--threshold <percent>]
                          [--time-threshold <percent>] [--json]
  mln-bench fixtures [--from <dir>]

--filter keeps the scenarios whose names contain one of the strings.
run and compare run each scenario in a process of its own (mln-bench exec).
compare builds this checkout's harness against <ref> and compares the two
(tools/bench/README.md).
fixtures installs the snapshot that tools/bench/fixtures.json names, from its
base URL or from a directory laid out as it is.";

fn main() -> ExitCode {
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
    threshold: f64,
    time_threshold: f64,
    from: Option<PathBuf>,
    positional: Vec<String>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut options = Options {
        filters: Vec::new(),
        json: false,
        rounds: None,
        threshold: 1.0,
        time_threshold: 2.0,
        from: None,
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
            "--threshold" => options.threshold = number(value()?)?,
            "--time-threshold" => options.time_threshold = number(value()?)?,
            "--from" => options.from = Some(PathBuf::from(value()?)),
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
            let binary = std::env::current_exe().map_err(|error| error.to_string())?;
            let mut runs = Vec::new();
            for scenario in selected {
                eprintln!("mln-bench: {}", scenario.name);
                runs.push(runner::run_in_process(&binary, scenario.name)?);
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
                threshold: options.threshold,
                time_threshold: options.time_threshold,
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
            println!("snapshot {} is installed in {}", manifest.snapshot, dir.display());
            Ok(())
        }
        ["exec", name, out] => {
            let scenario = scenarios::find(name).ok_or_else(|| format!("no scenario {name}"))?;
            runner::exec(scenario, &PathBuf::from(out))
        }
        _ => Err(USAGE.to_owned()),
    }
}
