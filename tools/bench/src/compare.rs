//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref, both sides running this checkout's harness, or with
//! `--own-harness` each side its own.
//!
//! The ref is checked out in a Git worktree under `<target>/bench-compare`,
//! which the next comparison reuses, with this checkout's harness laid over
//! it: `tools/bench`, and the libraries' `bench` modules, the entry points
//! that micro scenarios call. Both sides build with this checkout's
//! toolchain and `profile.toml`, so they differ only in the libraries. When
//! the harness doesn't build against the ref, the comparison stops. Every
//! run reports its harness id (build.rs), and the comparison refuses runs
//! whose ids differ.
//!
//! With `--own-harness`, the ref builds as it is, its own harness included,
//! with this checkout's toolchain and `profile.toml`: for a ref whose
//! libraries this checkout's harness doesn't build against, such as one
//! from before a change to a `bench` module's API. The sides compare the
//! scenarios both harnesses have, by name, and the report warns that their
//! harnesses differ, so that a change can come from either harness.
//!
//! Counts: each side runs once per scenario, and the ref's runs are kept
//! under `<target>/bench-compare/samples`, by commit and `profile.toml`, to
//! measure how much a count moves from run to run (its noise) without
//! running the ref again next time; the ref runs again only until it has
//! [`SPREAD_RUNS`] of them.
//! A scenario whose counts move beyond their noise runs again on this
//! checkout, until it has [`CONFIRM_RUNS`], and its counts come from those
//! runs. The
//! instructions a scenario retires also move with where the compiler and
//! linker put its code, which any edit to the binary can change, by up to a
//! few percent in some scenarios: each ref is built a second time with the
//! harness's `layout-probe` feature, which only moves code, and the most
//! that any stored probe has moved a scenario's instructions counts as
//! their noise too. The report shows every metric's change and its noise,
//! and marks a count that moved by over [`NOISE_FACTOR`] times its noise,
//! however little: one that repeats exactly is marked for any change. The
//! reader weighs what matters.
//! Times: one run per side has no noise to hold them against, unless
//! `--rounds` asks for interleaved rounds, which a rank test compares: the
//! wall-clock time, and the GPU's where the device times it. Rounds run
//! alone among the machine's benchmark runs ([`host::measuring`]), and not
//! while its load is over `--max-load`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::report::{Run, format_value, table};
use crate::runner::{run_in_process, warm_up};
use crate::scenario::{MetricKind, Scenario, Unit};
use crate::stats::{holm, mann_whitney, median, range};
use crate::{host, scenarios};

/// The base's runs a count's noise needs at least.
const SPREAD_RUNS: usize = 3;
/// The base's runs kept for each scenario, the latest.
const KEPT_RUNS: usize = 8;
/// This checkout's runs of a scenario whose counts moved beyond their noise
/// in one.
const CONFIRM_RUNS: usize = 3;
/// How many times its noise a count must move by to be marked.
const NOISE_FACTOR: f64 = 2.0;
/// A time is marked when its Holm-adjusted p-value is below this.
const ALPHA: f64 = 0.05;
/// The times that rounds test, each on its own: the wall-clock time, and
/// the GPU's, which on a GPU can change while the wall-clock time, mostly
/// the CPU's work, doesn't. A scenario's other times move with them.
const TESTED_TIMES: [&str; 2] = ["wall", "gpu"];

pub(crate) struct Options {
    pub(crate) base: String,
    pub(crate) filters: Vec<String>,
    /// Interleaved rounds per side, for times; none for one run per side.
    pub(crate) rounds: Option<usize>,
    /// The load average over which rounds don't run.
    pub(crate) max_load: f64,
    /// Whether the ref runs its own harness instead of this checkout's.
    pub(crate) own_harness: bool,
}

/// Which way a metric moved, when it moved beyond its noise. Lower is
/// better for every metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Moved {
    More,
    Less,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetricComparison {
    pub(crate) name: String,
    pub(crate) kind: MetricKind,
    pub(crate) unit: Unit,
    /// The median of each side's runs; none where the side lacks the metric.
    pub(crate) base: Option<f64>,
    pub(crate) candidate: Option<f64>,
    /// The candidate's change from the base, as a fraction of the base.
    pub(crate) change: Option<f64>,
    /// A count's noise, as a fraction of the base: the larger range of
    /// either side's runs, or for instructions the most a stored layout
    /// probe moved them, if that is larger.
    pub(crate) noise: Option<f64>,
    /// A tested time's p-value with rounds, Holm-adjusted over every
    /// scenario of the comparison.
    pub(crate) p: Option<f64>,
    /// Which way the metric moved, if beyond its noise.
    pub(crate) moved: Option<Moved>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScenarioComparison {
    pub(crate) scenario: String,
    pub(crate) environment: Environment,
    /// Why the scenario's changes aren't held against their noise, if they
    /// aren't.
    pub(crate) not_comparable: Option<String>,
    /// The base's runs its counts used, and the candidate's.
    pub(crate) base_runs: usize,
    pub(crate) candidate_runs: usize,
    pub(crate) metrics: Vec<MetricComparison>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Comparison {
    pub(crate) base: String,
    pub(crate) base_commit: String,
    /// The harness ids of this checkout's runs and of the base's, which
    /// differ only with `--own-harness`.
    pub(crate) harness: String,
    pub(crate) base_harness: String,
    /// The scenarios of the filters that only the base's harness has, and
    /// that only this checkout's has, which aren't compared.
    pub(crate) only_base: Vec<String>,
    pub(crate) only_this: Vec<String>,
    pub(crate) rounds: Option<usize>,
    /// With rounds, where the system keeps a load average: the machine's
    /// before the rounds and after them. The one after counts the rounds'
    /// own work.
    pub(crate) load: Option<Load>,
    pub(crate) scenarios: Vec<ScenarioComparison>,
}

/// The machine's load average around a comparison's rounds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Load {
    pub(crate) before: f64,
    pub(crate) after: f64,
}

impl Comparison {
    /// How many metrics moved beyond their noise.
    pub(crate) fn moved(&self) -> usize {
        self.scenarios.iter().flat_map(|scenario| &scenario.metrics).filter(|metric| metric.moved.is_some()).count()
    }
}

/// Builds both sides, runs the scenarios `options.filters` selects and
/// compares them.
pub(crate) fn compare(options: &Options) -> Result<Comparison, String> {
    let selected = scenarios::select(&options.filters);
    if selected.is_empty() {
        return Err("no scenario matches the filters".to_owned());
    }
    if let Some(rounds) = options.rounds {
        let needed = min_rounds(selected.len());
        if rounds < needed {
            return Err(format!(
                "--rounds {rounds} can't tell a time's change from noise among {} scenarios: that takes at least {needed} \
                 rounds, or fewer scenarios (--filter)",
                selected.len()
            ));
        }
    }
    // The checkout the comparison runs in, found at run time: a path built
    // into the binary would differ between the sides' builds, and move the
    // data that their code addresses.
    let root = PathBuf::from(git(Path::new("."), &["rev-parse", "--show-toplevel"])?);
    if !root.join("tools/bench/profile.toml").exists() {
        return Err(format!("{} has no tools/bench to compare with", root.display()));
    }
    let commit = git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", options.base)])?;
    let own = options.own_harness;

    eprintln!("mln-bench: building this checkout");
    let candidate = build(&root, &root.join("Cargo.toml"), None, None)?;
    // <target>/benchmark/mln-bench: the comparison works in <target>.
    let target = candidate.parent().and_then(Path::parent).ok_or("the binary's target directory")?;
    let work = target.join("bench-compare");
    std::fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let _lock = lock(&work)?;

    let whose = if own { "its own" } else { "this checkout's" };
    eprintln!("mln-bench: building {} ({}) with {whose} harness", options.base, short(&commit));
    let tree = work.join("tree");
    prepare_tree(&root, &tree, &commit, !own)?;
    let base_target = work.join("target");
    let base = build(&root, &tree.join("Cargo.toml"), Some(&base_target), None).map_err(|error| {
        if own {
            format!(
                "{error}\n\nThe harness of {} ({}) doesn't build with this checkout's toolchain and profile.toml.",
                options.base,
                short(&commit)
            )
        } else {
            format!(
                "{error}\n\nThis checkout's harness, with its libraries' bench modules, doesn't build against {} ({}). \
                 Compare with each side's own harness instead (--own-harness), or commit the library code a scenario \
                 or entry point needs with it, then compare later changes with that commit (tools/bench/README.md).",
                options.base,
                short(&commit)
            )
        }
    })?;
    // Both builds of the ref link to the same path, so each is copied out.
    let base = keep_binary(&base, &work.join("bin/base"))?;
    let probe = if own && !has_layout_probe(&tree) {
        eprintln!(
            "mln-bench: the harness of {} has no layout probe, so its instructions' noise comes from the probes of \
             refs compared before",
            options.base
        );
        None
    } else {
        eprintln!("mln-bench: building {} ({}) again, with its code at other addresses", options.base, short(&commit));
        let probe = build(&root, &tree.join("Cargo.toml"), Some(&base_target), Some("layout-probe"))?;
        Some(keep_binary(&probe, &work.join("bin/probe"))?)
    };
    let (scenarios, only_base, only_this) = if own {
        match_scenarios(&selected, &scenario_names(&base, &options.filters)?)
    } else {
        (selected, vec![], vec![])
    };
    for (side, names) in [(options.base.as_str(), &only_base), ("this checkout", &only_this)] {
        if !names.is_empty() {
            eprintln!("mln-bench: only {side}'s harness has {}, which isn't compared", names.join(", "));
        }
    }
    if scenarios.is_empty() {
        return Err(format!("no scenario of the filters is in both harnesses, this checkout's and {}'s", options.base));
    }

    let _measuring = host::measuring(options.rounds.is_some())?;
    let load_before = options.rounds.and_then(|_| host::load_average());
    if let Some(load) = load_before
        && load > options.max_load
    {
        return Err(format!(
            "the machine's load average is {load:.1}, over --max-load {:.1}: times measured now would mislead. \
             Run the rounds when the machine is quiet, or compare counts without --rounds",
            options.max_load
        ));
    }

    let profile = read(&root.join("tools/bench/profile.toml"))?;
    let samples_path = work.join("samples").join(samples_name(&commit, &profile));
    let mut samples = read_samples(&samples_path);
    eprintln!("mln-bench: a first run of each scenario on each side, not counted");
    let base_harness = harness_of(&warm_up(&base, &scenarios)?)?;
    let harness = harness_of(&warm_up(&candidate, &scenarios)?)?;
    if base_harness != harness {
        if !own {
            return Err(format!(
                "the sides ran different harness builds ({base_harness} and {harness}), so they can't be compared"
            ));
        }
        let changes = git(&root, &["diff", "--stat", &commit, "--", "tools/bench", "crates/*/src/bench.rs"])?;
        eprintln!(
            "mln-bench: warning: the sides run different harnesses, so a change can come from the harness as well as \
             the libraries:\n {changes}"
        );
    }
    let probe_key = |scenario: &Scenario| format!("{} (layout probe)", scenario.name);
    if let Some(probe) = &probe {
        let unprobed: Vec<&Scenario> = scenarios
            .iter()
            .copied()
            .filter(|scenario| {
                samples.get(&probe_key(scenario)).is_none_or(|runs| !runs.iter().any(|run| run.harness == base_harness))
            })
            .collect();
        if !unprobed.is_empty() {
            eprintln!("mln-bench: the ref's layout probe, once per scenario");
            warm_up(probe, &unprobed)?;
            for scenario in &unprobed {
                let run = run_in_process(probe, scenario.name, None)?;
                samples.insert(probe_key(scenario), vec![run]);
            }
        }
    }
    let changed_while_running =
        |scenario: &Scenario| format!("{}: a side's harness changed while it ran", scenario.name);
    let mut comparisons = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let (base_runs, mut candidate_runs) = run_sides(scenario, index, &base, &candidate, options.rounds)?;
        if base_runs.iter().any(|run| run.harness != base_harness)
            || candidate_runs.iter().any(|run| run.harness != harness)
        {
            return Err(changed_while_running(scenario));
        }
        let kept = samples.entry(scenario.name.to_owned()).or_default();
        let environment = &base_runs[0].environment;
        kept.retain(|run| run.harness == base_harness && &run.environment == environment);
        let mut base_counts = kept.clone();
        base_counts.extend(base_runs.iter().cloned());
        while base_counts.len() < SPREAD_RUNS {
            eprintln!("mln-bench: {}: another base run, for the noise", scenario.name);
            base_counts.push(run_in_process(&base, scenario.name, None)?);
        }
        let fresh = base_counts.len() - kept.len();
        kept.extend(base_counts[base_counts.len() - fresh..].iter().cloned());
        let excess = kept.len().saturating_sub(KEPT_RUNS);
        kept.drain(..excess);
        write_samples(&samples_path, &samples)?;
        let layout = layout_noise(&work.join("samples"), scenario.name, environment);
        let base = Base { runs: &base_runs, counts: &base_counts, layout: &layout };
        let mut comparison = compare_scenario(scenario, &base, &candidate_runs, options.rounds);
        if options.rounds.is_none() && comparison.has_count_moved() {
            while candidate_runs.len() < CONFIRM_RUNS {
                eprintln!("mln-bench: {}: this checkout again, to confirm its changes", scenario.name);
                let run = run_in_process(&candidate, scenario.name, None)?;
                if run.harness != harness {
                    return Err(changed_while_running(scenario));
                }
                candidate_runs.push(run);
            }
            comparison = compare_scenario(scenario, &base, &candidate_runs, options.rounds);
        }
        comparisons.push(comparison);
    }
    if options.rounds.is_some() {
        judge_times(&mut comparisons);
    }
    Ok(Comparison {
        base: options.base.clone(),
        base_commit: commit,
        harness,
        base_harness,
        only_base,
        only_this,
        rounds: options.rounds,
        load: load_before.zip(host::load_average()).map(|(before, after)| Load { before, after }),
        scenarios: comparisons,
    })
}

/// The harness id that every one of `runs`, a binary's, reports.
fn harness_of(runs: &[Run]) -> Result<String, String> {
    let harness = runs.first().map(|run| run.harness.clone()).ok_or("no runs")?;
    match runs.iter().find(|run| run.harness != harness) {
        Some(run) => Err(format!("one binary reported two harness ids, {harness} and {}", run.harness)),
        None => Ok(harness),
    }
}

/// The names of the scenarios that `filters` select in the harness `binary`.
fn scenario_names(binary: &Path, filters: &[String]) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Listed {
        name: String,
    }
    let mut command = Command::new(binary);
    command.args(["list", "--json"]);
    for filter in filters {
        command.args(["--filter", filter]);
    }
    let output = command.stderr(Stdio::inherit()).output().map_err(|error| format!("{}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!("{} list failed ({})", binary.display(), output.status));
    }
    let listed: Vec<Listed> =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("{} list: {error}", binary.display()))?;
    Ok(listed.into_iter().map(|listed| listed.name).collect())
}

/// The scenarios of `selected`, this checkout's, that the base's harness has
/// too, by name, in this checkout's order; then the names of those only the
/// base has, and of those only this checkout has.
fn match_scenarios<'a>(selected: &[&'a Scenario], base: &[String]) -> (Vec<&'a Scenario>, Vec<String>, Vec<String>) {
    let (both, only_this): (Vec<&Scenario>, Vec<&Scenario>) =
        selected.iter().partition(|scenario| base.iter().any(|name| name == scenario.name));
    let only_base =
        base.iter().filter(|name| selected.iter().all(|scenario| scenario.name != *name)).cloned().collect();
    (both, only_base, only_this.iter().map(|scenario| scenario.name.to_owned()).collect())
}

/// Whether the harness of the checkout at `tree` has the `layout-probe`
/// feature.
fn has_layout_probe(tree: &Path) -> bool {
    std::fs::read_to_string(tree.join("tools/bench/Cargo.toml")).is_ok_and(|manifest| manifest.contains("layout-probe"))
}

/// The fewest rounds with which a time can move beyond its noise when `tests`
/// scenarios each test it: Holm's adjustment multiplies the smallest
/// p-value by `tests`, and with n rounds per side the smallest p-value of a
/// two-sided Mann-Whitney U test is 2 / C(2n, n), when the sides don't
/// overlap at all.
fn min_rounds(tests: usize) -> usize {
    (2..=64)
        .find(|&n| {
            let orderings: f64 = (1..=n).map(|i| (n + i) as f64 / i as f64).product();
            2.0 / orderings * (tests as f64) < ALPHA
        })
        .unwrap_or(usize::MAX)
}

/// Gives each tested time of each scenario of `comparisons` its p-value
/// Holm-adjusted over all the scenarios, so that the chance of marking that
/// time anywhere when it didn't change stays under [`ALPHA`], and marks it
/// if it moved. Each tested time is a question of its own, and adjusting
/// over them all would raise the rounds a comparison needs.
fn judge_times(comparisons: &mut [ScenarioComparison]) {
    for name in TESTED_TIMES {
        let mut tested: Vec<&mut MetricComparison> = comparisons
            .iter_mut()
            .filter(|scenario| scenario.not_comparable.is_none())
            .flat_map(|scenario| scenario.metrics.iter_mut())
            .filter(|metric| metric.name == name && metric.kind == MetricKind::Time && metric.p.is_some())
            .collect();
        let p_values: Vec<f64> = tested.iter().map(|metric| metric.p.unwrap_or(1.0)).collect();
        for (metric, p) in tested.iter_mut().zip(holm(&p_values)) {
            metric.p = Some(p);
            if let (Some(b), Some(c), Some(change)) = (metric.base, metric.candidate, metric.change)
                && p < ALPHA
                && change != 0.0
            {
                metric.moved = Some(if c > b { Moved::More } else { Moved::Less });
            }
        }
    }
}

/// Runs `scenario` on both sides: once each, or `rounds` times each in
/// ABBA order, so that drift in the machine's load hits both alike. With
/// one run each, the side that goes first alternates between scenarios.
fn run_sides(
    scenario: &Scenario,
    index: usize,
    base: &Path,
    candidate: &Path,
    rounds: Option<usize>,
) -> Result<(Vec<Run>, Vec<Run>), String> {
    let mut base_runs = Vec::new();
    let mut candidate_runs = Vec::new();
    let rounds_count = rounds.unwrap_or(1);
    for round in 0..rounds_count {
        let base_first = match rounds {
            Some(_) => round % 4 == 0 || round % 4 == 3,
            None => index.is_multiple_of(2),
        };
        for is_base in [base_first, !base_first] {
            let side = if is_base { "base" } else { "this checkout" };
            eprintln!("mln-bench: {}: {side}, round {} of {rounds_count}", scenario.name, round + 1);
            if is_base {
                base_runs.push(run_in_process(base, scenario.name, None)?);
            } else {
                candidate_runs.push(run_in_process(candidate, scenario.name, None)?);
            }
        }
    }
    Ok((base_runs, candidate_runs))
}

/// The base's runs of one scenario.
struct Base<'a> {
    /// This comparison's, for times.
    runs: &'a [Run],
    /// These and kept ones, for counts.
    counts: &'a [Run],
    /// The most that a stored layout probe moved each of the scenario's
    /// instruction counts, as a fraction of the base ([`layout_noise`]).
    layout: &'a BTreeMap<String, f64>,
}

/// How far moving the code moves each instruction count of `scenario` in
/// `environment`: for each, the largest fraction of the base's median that
/// the layout probe of any ref compared so far moved it by, from the kept
/// runs in `samples`. How far it moves differs from build to build, so one
/// probe alone tells too little.
fn layout_noise(samples: &Path, scenario: &str, environment: &Environment) -> BTreeMap<String, f64> {
    let mut noise: BTreeMap<String, f64> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(samples) else { return noise };
    for entry in entries.flatten() {
        let kept = read_samples(&entry.path());
        let of = |key: &str| -> Vec<&Run> {
            kept.get(key).into_iter().flatten().filter(|run| &run.environment == environment).collect()
        };
        let (base, probe) = (of(scenario), of(&format!("{scenario} (layout probe)")));
        let Some(probe) = probe.first() else { continue };
        // Only the base's runs of the probe's harness: another harness's
        // differ by more than where the code lies.
        let base: Vec<&Run> = base.into_iter().filter(|run| run.harness == probe.harness).collect();
        for metric in probe.metrics.iter().filter(|metric| metric.name.starts_with("instructions")) {
            let values: Vec<f64> = base.iter().filter_map(|run| run.metric(&metric.name)).map(|m| m.value).collect();
            if values.is_empty() {
                continue;
            }
            let median = median(&values);
            let moved = relative(metric.value - median, median).abs();
            let largest = noise.entry(metric.name.clone()).or_default();
            *largest = largest.max(moved);
        }
    }
    noise
}

/// The comparison of one scenario's runs: `base`'s and `candidate_runs`,
/// this comparison's. With rounds, the tested time gets its p-value
/// unadjusted, and [`judge_times`] judges it.
fn compare_scenario(
    scenario: &Scenario,
    base_side: &Base<'_>,
    candidate_runs: &[Run],
    rounds: Option<usize>,
) -> ScenarioComparison {
    let (base_runs, base_counts) = (base_side.runs, base_side.counts);
    let environment = candidate_runs[0].environment.clone();
    let mut not_comparable = None;
    if let Some(run) = base_runs.iter().chain(candidate_runs).find(|run| run.environment != environment) {
        not_comparable = Some(format!(
            "the sides ran in different environments ({})",
            environment.differences(&run.environment).join("; ")
        ));
    } else if timed_gpu(base_counts) != timed_gpu(candidate_runs) {
        // A commit from before the libraries timed the GPU, against one
        // that does.
        not_comparable = Some("only one side timed the GPU, whose timestamps cost work of their own".to_owned());
    }
    let values = |runs: &[Run], name: &str| -> Vec<f64> {
        runs.iter().filter_map(|run| run.metric(name)).map(|metric| metric.value).collect()
    };
    // Every metric either side reported, in the candidate's order.
    let mut names: Vec<(&str, MetricKind, Unit)> = Vec::new();
    for run in candidate_runs.iter().chain(base_runs) {
        for metric in &run.metrics {
            if names.iter().all(|(name, ..)| *name != metric.name) {
                names.push((&metric.name, metric.kind, metric.unit));
            }
        }
    }
    let mut metrics: Vec<MetricComparison> = Vec::new();
    for (name, kind, unit) in names {
        let base = if kind == MetricKind::Count { values(base_counts, name) } else { values(base_runs, name) };
        let candidate = values(candidate_runs, name);
        let base_median = (!base.is_empty()).then(|| median(&base));
        let candidate_median = (!candidate.is_empty()).then(|| median(&candidate));
        let change = base_median.zip(candidate_median).map(|(b, c)| relative(c - b, b));
        let mut comparison = MetricComparison {
            name: name.to_owned(),
            kind,
            unit,
            base: base_median,
            candidate: candidate_median,
            change,
            noise: None,
            p: None,
            moved: None,
        };
        if let (Some(b), Some(c)) = (base_median, candidate_median)
            && kind == MetricKind::Count
        {
            let mut spread = range(&base).max(range(&candidate));
            if let Some(moved) = base_side.layout.get(name) {
                spread = spread.max(moved * b.abs());
            }
            comparison.noise = Some(relative(spread, b));
            if (c - b).abs() > NOISE_FACTOR * spread {
                comparison.moved = Some(if c > b { Moved::More } else { Moved::Less });
            }
        }
        if rounds.is_some() && kind == MetricKind::Time && TESTED_TIMES.contains(&name) {
            comparison.p = (!base.is_empty() && !candidate.is_empty()).then(|| mann_whitney(&base, &candidate));
        }
        metrics.push(comparison);
    }
    if not_comparable.is_some() {
        for metric in &mut metrics {
            metric.moved = None;
        }
    }
    ScenarioComparison {
        scenario: scenario.name.to_owned(),
        environment,
        not_comparable,
        base_runs: base_counts.len(),
        candidate_runs: candidate_runs.len(),
        metrics,
    }
}

/// Whether `runs` timed the GPU's render passes, which costs instructions
/// and allocations of its own.
fn timed_gpu(runs: &[Run]) -> bool {
    runs.iter().any(|run| run.metric("gpu").is_some())
}

impl ScenarioComparison {
    fn has_count_moved(&self) -> bool {
        self.metrics.iter().any(|metric| metric.kind == MetricKind::Count && metric.moved.is_some())
    }
}

/// `delta` as a fraction of `base`: 0 for no change, infinite for a change
/// from zero.
fn relative(delta: f64, base: f64) -> f64 {
    if delta == 0.0 {
        0.0
    } else if base == 0.0 {
        f64::INFINITY.copysign(delta)
    } else {
        delta / base.abs()
    }
}

/// The comparison, for reading.
pub(crate) fn text(comparison: &Comparison) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "mln-bench compare: this checkout against {} ({})",
        comparison.base,
        short(&comparison.base_commit)
    );
    if comparison.base_harness == comparison.harness {
        let _ = writeln!(out, "harness {} on both sides", comparison.harness);
    } else {
        let _ = writeln!(
            out,
            "harness {} on the base and {} on this checkout, each side's own: a change can come from either harness",
            comparison.base_harness, comparison.harness
        );
    }
    for (side, names) in [("the base", &comparison.only_base), ("this checkout", &comparison.only_this)] {
        if !names.is_empty() {
            let _ = writeln!(out, "only {side}'s harness has, not compared: {}", names.join(", "));
        }
    }
    let _ = writeln!(
        out,
        "noise: a count's larger range over either side's runs, and for instructions the most a build laid out \
         differently moved them; a change over {NOISE_FACTOR}x its noise is marked more or less"
    );
    match comparison.rounds {
        Some(rounds) => {
            let _ = writeln!(
                out,
                "times: {rounds} interleaved rounds; {} each marked when its Mann-Whitney p, Holm-adjusted over \
                 the scenarios, is under {ALPHA}",
                TESTED_TIMES.join(" and ")
            );
        }
        None => {
            let _ = writeln!(out, "times: one run per side, so no noise to hold them against (--rounds <n>)");
        }
    }
    if let Some(load) = comparison.load {
        let _ = writeln!(
            out,
            "load average: {:.1} before the rounds, {:.1} after, which counts the rounds' own work",
            load.before, load.after
        );
    }
    let mut environments: Vec<&Environment> = Vec::new();
    for scenario in &comparison.scenarios {
        if !environments.contains(&&scenario.environment) {
            environments.push(&scenario.environment);
        }
    }
    for environment in environments {
        let _ = writeln!(out, "environment: {environment}");
    }
    for scenario in &comparison.scenarios {
        let _ = writeln!(
            out,
            "\n{} (counts: {} base runs, {} of this checkout)",
            scenario.scenario, scenario.base_runs, scenario.candidate_runs
        );
        if let Some(reason) = &scenario.not_comparable {
            let _ = writeln!(out, "  not comparable: {reason}");
        }
        let mut rows = vec![["metric", "base", "this", "change", "noise", "moved"].map(str::to_owned).to_vec()];
        for metric in &scenario.metrics {
            let value =
                |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format_value(metric.unit, value));
            let noise = match (metric.noise, metric.p) {
                (Some(noise), _) => percent(noise, false),
                (None, Some(p)) => format!("p={p:.3}"),
                (None, None) => String::new(),
            };
            let moved = match metric.moved {
                Some(Moved::More) => "more",
                Some(Moved::Less) => "less",
                None => "",
            };
            rows.push(vec![
                metric.name.clone(),
                value(metric.base),
                value(metric.candidate),
                metric.change.map_or_else(String::new, |change| percent(change, true)),
                noise,
                moved.to_owned(),
            ]);
        }
        out.push_str(&table(&rows));
    }
    let moved = comparison.moved();
    let _ = writeln!(out, "\n{moved} change{} beyond noise", if moved == 1 { "" } else { "s" });
    out
}

fn percent(fraction: f64, signed: bool) -> String {
    if fraction.is_infinite() {
        return if fraction > 0.0 { "new".to_owned() } else { "gone".to_owned() };
    }
    let value = fraction * 100.0;
    let digits = if value.abs() < 1.0 { 2 } else { 1 };
    if signed { format!("{value:+.digits$}%") } else { format!("{value:.digits$}%") }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Builds the harness of the workspace `manifest` with the benchmark profile
/// of this checkout, with this checkout's toolchain, and the harness's
/// `features`, and returns the binary.
fn build(root: &Path, manifest: &Path, target_dir: Option<&Path>, features: Option<&str>) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    // The checkout's directory, whose rust-toolchain.toml rustup reads.
    command
        .current_dir(root)
        .args(["build", "--profile", "benchmark", "--config"])
        .arg(root.join("tools/bench/profile.toml"))
        .args(["-p", "mln-rs-bench", "--bin", "mln-bench", "--message-format", "json-render-diagnostics"])
        .arg("--manifest-path")
        .arg(manifest);
    if let Some(dir) = target_dir {
        // A base's build, the only one in a directory of its own. Its code
        // can go unused under this checkout's harness, such as a `bench`
        // hook that its libraries don't call, and its warnings are no one's
        // to fix, even in CI, which denies them.
        command.arg("--target-dir").arg(dir).env("CARGO_BUILD_WARNINGS", "allow");
    }
    if let Some(features) = features {
        command.args(["--features", features]);
    }
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!("building {} failed", manifest.display()));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact" && message["target"]["name"] == "mln-bench")
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| format!("building {} gave no mln-bench binary", manifest.display()))
}

/// Copies the binary `built` to `kept`, unless it holds the same already,
/// and returns `kept`.
fn keep_binary(built: &Path, kept: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = kept.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    write_if_changed(kept, &read(built)?)?;
    let permissions = std::fs::metadata(built).map_err(|error| format!("{}: {error}", built.display()))?.permissions();
    std::fs::set_permissions(kept, permissions).map_err(|error| format!("{}: {error}", kept.display()))?;
    Ok(kept.to_owned())
}

/// Checks `commit` out at `tree`, a worktree of the checkout's repository,
/// with `tools/bench` a member of its workspace, and with `overlay` lays
/// this checkout's harness over it: `tools/bench`, and each library's
/// `bench` module, in place of the commit's. A file is written only where
/// its contents differ, so that Cargo builds again only what changed since
/// the last comparison.
fn prepare_tree(root: &Path, tree: &Path, commit: &str, overlay: bool) -> Result<(), String> {
    if tree.join(".git").exists() {
        // With the overlay, the tree keeps the harness of the last
        // comparison when it has the commit already. Without, it goes back
        // to the commit's: checking out writes only the files that differ.
        if !overlay || git(tree, &["rev-parse", "HEAD"]).ok().as_deref() != Some(commit) {
            git(tree, &["checkout", "-q", "--force", "--detach", commit])?;
            git(tree, &["clean", "-q", "-d", "--force"])?;
        }
    } else {
        if tree.exists() {
            std::fs::remove_dir_all(tree).map_err(|error| format!("{}: {error}", tree.display()))?;
        }
        let path = tree.to_str().ok_or("a path that isn't UTF-8")?;
        git(root, &["worktree", "add", "-q", "--force", "--detach", path, commit])?;
    }
    if overlay {
        sync_dir(&root.join("tools/bench"), &tree.join("tools/bench"))?;
        let modules = bench_modules(root)?;
        for module in &modules {
            let path = tree.join(module);
            if !path.parent().is_some_and(Path::is_dir) {
                return Err(format!(
                    "{} has no {}, so this checkout's harness, which calls its {}, can't build against it: compare \
                     with each side's own harness instead (--own-harness)",
                    short(commit),
                    module.parent().unwrap_or(module).display(),
                    module.display()
                ));
            }
            write_if_changed(&path, &read(&root.join(module))?)?;
        }
        // A module that only the commit has goes back to the commit's.
        for module in bench_modules(tree)? {
            if !modules.contains(&module) {
                let path = module.to_str().ok_or("a path that isn't UTF-8")?.replace('\\', "/");
                if git(tree, &["checkout", "-q", commit, "--", &path]).is_err() {
                    let path = tree.join(&module);
                    std::fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                }
            }
        }
    }
    let manifest_path = tree.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    write_if_changed(&manifest_path, with_member(&manifest)?.as_bytes())
}

/// The libraries' `bench` modules in the checkout at `root`, which belong to
/// the harness: `crates/<library>/src/bench.rs`, relative to `root`.
fn bench_modules(root: &Path) -> Result<Vec<PathBuf>, String> {
    let crates = root.join("crates");
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(&crates).map_err(|error| format!("{}: {error}", crates.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let module = Path::new("crates").join(entry.file_name()).join("src/bench.rs");
        if root.join(&module).is_file() {
            modules.push(module);
        }
    }
    modules.sort();
    Ok(modules)
}

/// The workspace manifest `manifest` with `tools/bench` among its members.
fn with_member(manifest: &str) -> Result<String, String> {
    if manifest.contains("\"tools/bench\"") {
        return Ok(manifest.to_owned());
    }
    let members = "members = [";
    let at = manifest.find(members).ok_or("the base's Cargo.toml lists no workspace members")? + members.len();
    Ok(format!("{}\n  \"tools/bench\",{}", &manifest[..at], &manifest[at..]))
}

/// Makes the directory `to` hold what `from` holds, leaving out build
/// output, and writes only the files whose contents differ.
fn sync_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if entry.file_name() == "target" {
            continue;
        }
        if path.is_dir() {
            sync_dir(&path, &to.join(entry.file_name()))?;
        } else {
            write_if_changed(&to.join(entry.file_name()), &read(&path)?)?;
        }
        names.push(entry.file_name());
    }
    for entry in std::fs::read_dir(to).map_err(|error| format!("{}: {error}", to.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() == "target" || names.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Writes `contents` to `path` unless it holds them already.
fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

/// Holds the comparison's lock in `work` until it drops, so that two
/// comparisons from one checkout take turns with its worktree.
fn lock(work: &Path) -> Result<File, String> {
    let path = work.join("lock");
    let file = File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.try_lock().is_err() {
        eprintln!("mln-bench: waiting for another comparison in this checkout");
        file.lock().map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(file)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

type Samples = BTreeMap<String, Vec<Run>>;

/// The file of the base's kept runs: by commit, and by `profile`, the
/// `profile.toml` that both sides build with, which the base's harness id
/// covers only when the harness is this checkout's.
fn samples_name(commit: &str, profile: &[u8]) -> String {
    // FNV-1a, which every build of the tool computes the same way.
    let hash = profile
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3));
    format!("{commit}-{hash:016x}.json")
}

/// The base's kept runs, or none when there are none or they don't read.
fn read_samples(path: &Path) -> Samples {
    std::fs::read_to_string(path).ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

fn write_samples(path: &Path, samples: &Samples) -> Result<(), String> {
    let dir = path.parent().ok_or("the samples' directory")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let json = serde_json::to_string(samples).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{Kind, Metric};

    fn run(instructions: f64, wall: f64) -> Run {
        run_with(instructions, 1000.0, wall)
    }

    fn run_with(instructions: f64, allocations: f64, wall: f64) -> Run {
        let count = |name: &str, kind, value| Metric { name: name.to_owned(), kind, unit: Unit::Count, value };
        Run {
            scenario: "micro/test/case".to_owned(),
            kind: Kind::Micro,
            harness: "h".to_owned(),
            environment: Environment::machine(),
            metrics: vec![
                count("instructions", MetricKind::Count, instructions),
                count("allocations", MetricKind::Count, allocations),
                count("allocations_per_tile", MetricKind::Count, allocations / 10.0),
                Metric { name: "wall".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
                Metric { name: "cpu".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
                Metric { name: "gpu".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: 1.0 },
            ],
        }
    }

    const SCENARIO: Scenario = Scenario { name: "micro/test/case", kind: Kind::Micro, question: "?", run: |_| {} };

    fn moved(comparison: &ScenarioComparison, name: &str) -> Option<Moved> {
        comparison.metrics.iter().find(|metric| metric.name == name).unwrap().moved
    }

    fn base<'a>(runs: &'a [Run], layout: &'a BTreeMap<String, f64>) -> Base<'a> {
        Base { runs: &runs[..1], counts: runs, layout }
    }

    #[test]
    fn a_count_moves_beyond_twice_its_noise() {
        let none = BTreeMap::new();
        let runs = [run(1000.0, 1.0), run(1001.0, 1.0), run(1002.0, 1.0)];
        // +0.5% of the median, 1001: over twice the noise of 2.
        let comparison = compare_scenario(&SCENARIO, &base(&runs, &none), &[run(1006.0, 5.0)], None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::More));
        // Times have no noise to hold against without rounds.
        assert_eq!(moved(&comparison, "wall"), None);
        // +0.2%: within it.
        let comparison = compare_scenario(&SCENARIO, &base(&runs, &none), &[run(1003.0, 1.0)], None);
        assert_eq!(moved(&comparison, "instructions"), None);
        // A noise of 30 hides a change of 50.
        let noisy = [run(1000.0, 1.0), run(1015.0, 1.0), run(1030.0, 1.0)];
        let comparison = compare_scenario(&SCENARIO, &base(&noisy, &none), &[run(1065.0, 1.0)], None);
        assert_eq!(moved(&comparison, "instructions"), None);
        let comparison = compare_scenario(&SCENARIO, &base(&noisy, &none), &[run(900.0, 1.0)], None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::Less));
    }

    #[test]
    fn instructions_take_the_layout_noise_as_noise() {
        let runs = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        // +2%, where moving the code has moved the count by 3%.
        let candidate = [run(1020.0, 1.0)];
        let layout = BTreeMap::from([("instructions".to_owned(), 0.03)]);
        let comparison = compare_scenario(&SCENARIO, &base(&runs, &layout), &candidate, None);
        assert_eq!(moved(&comparison, "instructions"), None);
        let comparison = compare_scenario(&SCENARIO, &base(&runs, &BTreeMap::new()), &candidate, None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::More));
    }

    #[test]
    fn a_count_that_repeats_moves_beyond_its_noise_with_any_change() {
        let runs = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        // One allocation more, and its mean per tile with it.
        let comparison =
            compare_scenario(&SCENARIO, &base(&runs, &BTreeMap::new()), &[run_with(1000.0, 1001.0, 1.0)], None);
        assert_eq!(moved(&comparison, "allocations"), Some(Moved::More));
        assert_eq!(moved(&comparison, "instructions"), None);
        assert_eq!(moved(&comparison, "allocations_per_tile"), Some(Moved::More));
        assert!(comparison.has_count_moved());
    }

    #[test]
    fn layout_noise_is_the_most_any_stored_probe_moved_a_count() {
        let dir = tempfile::tempdir().unwrap();
        let environment = Environment::machine();
        for (commit, probe) in [("a", 1010.0), ("b", 1030.0)] {
            let samples = Samples::from([
                ("micro/test/case".to_owned(), vec![run(1000.0, 1.0), run(1000.0, 1.0)]),
                ("micro/test/case (layout probe)".to_owned(), vec![run(probe, 1.0)]),
            ]);
            write_samples(&dir.path().join(format!("{commit}.json")), &samples).unwrap();
        }
        let noise = layout_noise(dir.path(), "micro/test/case", &environment);
        assert!((noise["instructions"] - 0.03).abs() < 1e-9);
        assert!(!noise.contains_key("allocations"));
    }

    #[test]
    fn a_time_needs_rounds_and_a_significant_change_to_move() {
        let base: Vec<Run> = (0..6).map(|i| run(1000.0, 10.0 + f64::from(i) * 0.01)).collect();
        let slower: Vec<Run> = (0..6).map(|i| run(1000.0, 11.0 + f64::from(i) * 0.01)).collect();
        let same: Vec<Run> = (0..6).map(|i| run(1000.0, 10.005 + f64::from(i) * 0.01)).collect();
        let none = BTreeMap::new();
        let all = |runs| Base { runs, counts: runs, layout: &none };
        let mut comparisons = vec![
            compare_scenario(&SCENARIO, &all(&base), &slower, Some(6)),
            compare_scenario(&SCENARIO, &all(&base), &same, Some(6)),
        ];
        judge_times(&mut comparisons);
        assert_eq!(moved(&comparisons[0], "wall"), Some(Moved::More));
        // Only wall and gpu are tested.
        assert_eq!(moved(&comparisons[0], "cpu"), None);
        assert_eq!(moved(&comparisons[1], "wall"), None);
    }

    #[test]
    fn the_gpu_time_is_tested_on_its_own() {
        let with_gpu = |runs: &mut Vec<Run>, gpu: f64| {
            for (i, run) in runs.iter_mut().enumerate() {
                run.metrics.iter_mut().find(|metric| metric.name == "gpu").unwrap().value = gpu + i as f64 * 0.01;
            }
        };
        let mut base: Vec<Run> = (0..4).map(|_| run(1000.0, 10.0)).collect();
        let mut slower: Vec<Run> = (0..4).map(|_| run(1000.0, 10.0)).collect();
        with_gpu(&mut base, 1.0);
        with_gpu(&mut slower, 2.0);
        let none = BTreeMap::new();
        let mut comparisons =
            vec![compare_scenario(&SCENARIO, &Base { runs: &base, counts: &base, layout: &none }, &slower, Some(4))];
        judge_times(&mut comparisons);
        // Four rounds suffice for one scenario: the wall time's test doesn't
        // raise the gpu time's p-value.
        assert_eq!(moved(&comparisons[0], "gpu"), Some(Moved::More));
        assert_eq!(moved(&comparisons[0], "wall"), None);
    }

    #[test]
    fn rounds_grow_with_the_scenarios_tested() {
        // 2 / C(8, 4) = 0.029 for one scenario; 2 / C(14, 7) x 50 = 0.029.
        assert_eq!(min_rounds(1), 4);
        assert_eq!(min_rounds(2), 5);
        assert_eq!(min_rounds(50), 7);
    }

    #[test]
    fn different_environments_are_not_compared() {
        let none = BTreeMap::new();
        let base_of = |runs| base(runs, &none);
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut other = run(2000.0, 1.0);
        other.environment.cpu = "another".to_owned();
        let comparison = compare_scenario(&SCENARIO, &base_of(&base), &[other], None);
        assert!(comparison.not_comparable.is_some());
        assert_eq!(moved(&comparison, "instructions"), None);
    }

    #[test]
    fn a_side_that_didnt_time_the_gpu_is_not_compared() {
        let none = BTreeMap::new();
        let base_runs = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut untimed = run(1050.0, 1.0);
        untimed.metrics.retain(|metric| metric.name != "gpu");
        let comparison = compare_scenario(&SCENARIO, &base(&base_runs, &none), &[untimed], None);
        assert!(comparison.not_comparable.is_some());
        assert_eq!(moved(&comparison, "instructions"), None);
    }

    #[test]
    fn matches_the_sides_scenarios_by_name() {
        const fn scenario(name: &'static str) -> Scenario {
            Scenario { name, kind: Kind::Micro, question: "?", run: |_| {} }
        }
        let (a, b, c) = (scenario("micro/a"), scenario("micro/b"), scenario("micro/c"));
        let base = ["micro/c".to_owned(), "micro/old".to_owned(), "micro/a".to_owned()];
        let (both, only_base, only_this) = match_scenarios(&[&a, &b, &c], &base);
        assert_eq!(both.iter().map(|scenario| scenario.name).collect::<Vec<_>>(), ["micro/a", "micro/c"]);
        assert_eq!(only_base, ["micro/old"]);
        assert_eq!(only_this, ["micro/b"]);
    }

    /// Runs `git args` in `dir`, as a committer of its own.
    fn git_in(dir: &Path, args: &[&str]) -> String {
        let identity =
            ["-c", "user.name=mln-bench", "-c", "user.email=mln-bench@localhost", "-c", "commit.gpgsign=false"];
        let output = Command::new("git").arg("-C").arg(dir).args(identity).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn contents(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn prepares_the_refs_tree_with_this_checkouts_harness_or_its_own() {
        let (repository, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, tree) = (repository.path(), work.path().join("tree"));
        git_in(root, &["init", "-q"]);
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\n  \"crates/*\",\n]\n");
        write(&root.join("crates/lib/src/bench.rs"), "the ref's");
        write(&root.join("tools/bench/src/main.rs"), "the ref's");
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "the ref"]);
        let commit = git_in(root, &["rev-parse", "HEAD"]);
        write(&root.join("crates/lib/src/bench.rs"), "this checkout's");
        write(&root.join("tools/bench/src/main.rs"), "this checkout's");
        write(&root.join("tools/bench/src/new.rs"), "this checkout's");

        let (module, harness, new) = (
            tree.join("crates/lib/src/bench.rs"),
            tree.join("tools/bench/src/main.rs"),
            tree.join("tools/bench/src/new.rs"),
        );
        prepare_tree(root, &tree, &commit, false).unwrap();
        assert_eq!((contents(&module), contents(&harness)), ("the ref's".to_owned(), "the ref's".to_owned()));
        assert!(!new.exists());
        assert!(contents(&tree.join("Cargo.toml")).contains("\"tools/bench\""));

        prepare_tree(root, &tree, &commit, true).unwrap();
        assert_eq!(
            (contents(&module), contents(&harness)),
            ("this checkout's".to_owned(), "this checkout's".to_owned())
        );
        assert!(new.exists());

        // The tree of the same commit, used again without the overlay, goes
        // back to the ref's harness.
        prepare_tree(root, &tree, &commit, false).unwrap();
        assert_eq!((contents(&module), contents(&harness)), ("the ref's".to_owned(), "the ref's".to_owned()));
        assert!(!new.exists());
    }

    #[test]
    fn the_overlay_of_a_module_whose_crate_the_ref_lacks_points_to_the_refs_own_harness() {
        let (repository, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, tree) = (repository.path(), work.path().join("tree"));
        git_in(root, &["init", "-q"]);
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\n  \"crates/*\",\n]\n");
        write(&root.join("crates/lib/src/bench.rs"), "the ref's");
        write(&root.join("tools/bench/src/main.rs"), "the ref's");
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "the ref"]);
        let commit = git_in(root, &["rev-parse", "HEAD"]);
        write(&root.join("crates/new/src/bench.rs"), "this checkout's");

        let error = prepare_tree(root, &tree, &commit, true).unwrap_err();
        assert!(error.contains("--own-harness"), "{error}");
        prepare_tree(root, &tree, &commit, false).unwrap();
    }

    #[test]
    fn keeps_the_bases_runs_by_commit_and_profile() {
        let name = samples_name("abc", b"codegen-units = 1");
        assert_eq!(name, samples_name("abc", b"codegen-units = 1"));
        assert_ne!(name, samples_name("abc", b"codegen-units = 16"));
        assert_ne!(name, samples_name("abd", b"codegen-units = 1"));
    }

    #[test]
    fn adds_the_harness_to_the_workspace_once() {
        let manifest = "[workspace]\nmembers = [\n  \"crates/*\",\n]\n";
        let with = with_member(manifest).unwrap();
        assert!(with.contains("\"tools/bench\""));
        assert_eq!(with_member(&with).unwrap(), with);
    }
}
