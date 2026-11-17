//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref, both sides running this checkout's harness.
//!
//! The ref is checked out in a Git worktree under `<target>/bench-compare`,
//! which the next comparison reuses, with this checkout's harness laid over
//! it: `tools/bench`, and the libraries' `bench` modules, the entry points
//! that micro scenarios call. Both sides build with this checkout's
//! toolchain and `profile.toml`, so they differ only in the libraries. When
//! the harness doesn't build against the ref, the comparison stops: it never
//! runs the ref's own harness, whose scenarios may differ from this one's.
//! Every run reports its harness id (build.rs), and the comparison refuses
//! runs whose ids differ.
//!
//! Counts: each side runs once per scenario, and the ref's runs are kept
//! under `<target>/bench-compare/samples`, by commit, to measure how much a
//! count moves from run to run (its spread) without running the ref again
//! next time; the ref runs again only until it has [`SPREAD_RUNS`] of them.
//! A scenario whose counts get a verdict runs again on this checkout, until
//! it has [`CONFIRM_RUNS`], and its verdicts come from those runs. The
//! instructions a scenario retires also move with where the compiler and
//! linker put its code, which any edit to the binary can change: the ref is
//! built a second time with the harness's `layout-probe` feature, which only
//! moves code, and how far each scenario's instructions move there counts
//! in their spread.
//! Times: context only, unless `--rounds` asks for interleaved rounds, which
//! a rank test then judges.

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
use crate::scenarios;
use crate::stats::{holm, mann_whitney, median, range};

/// The base's runs a count's spread needs at least.
const SPREAD_RUNS: usize = 3;
/// The base's runs kept for each scenario, the latest.
const KEPT_RUNS: usize = 8;
/// This checkout's runs of a scenario whose counts got a verdict from one.
const CONFIRM_RUNS: usize = 3;
/// How many times the larger spread of the two sides a count must move by.
const SPREAD_FACTOR: f64 = 2.0;
/// A time's verdict needs its Holm-adjusted p-value below this.
const ALPHA: f64 = 0.05;
/// The time that gets a verdict with rounds; a scenario's other times are
/// context, since they move with it.
const TESTED_TIME: &str = "wall";

pub(crate) struct Options {
    pub(crate) base: String,
    pub(crate) filters: Vec<String>,
    /// Interleaved rounds per side, for times; none for one run per side.
    pub(crate) rounds: Option<usize>,
    /// The smallest change of a count that gets a verdict, in percent.
    pub(crate) threshold: f64,
    /// The same for instruction counts, which a change to how the compiler
    /// lays out code anywhere in the binary can move a little.
    pub(crate) instructions_threshold: f64,
    /// The smallest change of a time's median that gets a verdict, in
    /// percent.
    pub(crate) time_threshold: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Better,
    Worse,
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
    /// A count's spread, as a fraction of the base: the larger range of
    /// either side's runs, or for instructions how far they moved in the
    /// ref's layout probe, if that is larger.
    pub(crate) spread: Option<f64>,
    /// The tested time's p-value with rounds, Holm-adjusted over every
    /// scenario of the comparison.
    pub(crate) p: Option<f64>,
    pub(crate) verdict: Option<Verdict>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScenarioComparison {
    pub(crate) scenario: String,
    pub(crate) environment: Environment,
    /// Why the scenario gets no verdicts, if it doesn't.
    pub(crate) no_verdicts: Option<String>,
    /// The base's runs its counts used, and the candidate's.
    pub(crate) base_runs: usize,
    pub(crate) candidate_runs: usize,
    pub(crate) metrics: Vec<MetricComparison>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Comparison {
    pub(crate) base: String,
    pub(crate) base_commit: String,
    pub(crate) harness: String,
    pub(crate) rounds: Option<usize>,
    pub(crate) threshold: f64,
    pub(crate) instructions_threshold: f64,
    pub(crate) time_threshold: f64,
    pub(crate) scenarios: Vec<ScenarioComparison>,
}

impl Comparison {
    pub(crate) fn verdicts(&self) -> usize {
        self.scenarios.iter().flat_map(|scenario| &scenario.metrics).filter(|metric| metric.verdict.is_some()).count()
    }
}

/// Builds both sides, runs the scenarios `options.filters` selects and
/// compares them.
pub(crate) fn compare(options: &Options) -> Result<Comparison, String> {
    let scenarios = scenarios::select(&options.filters);
    if scenarios.is_empty() {
        return Err("no scenario matches the filters".to_owned());
    }
    if let Some(rounds) = options.rounds {
        let needed = min_rounds(scenarios.len());
        if rounds < needed {
            return Err(format!(
                "--rounds {rounds} can't give a time a verdict among {} scenarios: that takes at least {needed} \
                 rounds, or fewer scenarios (--filter)",
                scenarios.len()
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

    eprintln!("mln-bench: building this checkout");
    let candidate = build(&root, &root.join("Cargo.toml"), None, None)?;
    // <target>/benchmark/mln-bench: the comparison works in <target>.
    let target = candidate.parent().and_then(Path::parent).ok_or("the binary's target directory")?;
    let work = target.join("bench-compare");
    std::fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let _lock = lock(&work)?;

    eprintln!("mln-bench: building {} ({}) with this checkout's harness", options.base, short(&commit));
    let tree = work.join("tree");
    prepare_tree(&root, &tree, &commit)?;
    let base_target = work.join("target");
    let base = build(&root, &tree.join("Cargo.toml"), Some(&base_target), None).map_err(|error| {
        format!(
            "{error}\n\nThis checkout's harness, with its libraries' bench modules, doesn't build against {} ({}), \
             so there are no verdicts. A scenario or entry point that needs library code the base lacks can't be \
             compared with it: commit that code with the scenario, then compare later changes with that commit \
             (tools/bench/README.md).",
            options.base,
            short(&commit)
        )
    })?;
    // Both builds of the ref link to the same path, so each is copied out.
    let base = keep_binary(&base, &work.join("bin/base"))?;
    eprintln!("mln-bench: building {} ({}) again, with its code at other addresses", options.base, short(&commit));
    let probe = build(&root, &tree.join("Cargo.toml"), Some(&base_target), Some("layout-probe"))?;
    let probe = keep_binary(&probe, &work.join("bin/probe"))?;

    let samples_path = work.join("samples").join(format!("{commit}.json"));
    let mut samples = read_samples(&samples_path);
    eprintln!("mln-bench: a first run of each scenario on each side, not counted");
    warm_up(&base, &scenarios)?;
    warm_up(&candidate, &scenarios)?;
    let probe_key = |scenario: &Scenario| format!("{} (layout probe)", scenario.name);
    let unprobed: Vec<&Scenario> = scenarios
        .iter()
        .copied()
        .filter(|scenario| {
            samples
                .get(&probe_key(scenario))
                .is_none_or(|runs| !runs.iter().any(|run| run.harness == env!("MLN_BENCH_HARNESS")))
        })
        .collect();
    if !unprobed.is_empty() {
        eprintln!("mln-bench: the ref's layout probe, once per scenario");
        warm_up(&probe, &unprobed)?;
        for scenario in &unprobed {
            let run = run_in_process(&probe, scenario.name, None)?;
            samples.insert(probe_key(scenario), vec![run]);
        }
    }
    let mut comparisons = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let (base_runs, mut candidate_runs) = run_sides(scenario, index, &base, &candidate, options.rounds)?;
        let harness = candidate_runs[0].harness.clone();
        let harness = &harness;
        if let Some(run) = base_runs.iter().chain(&candidate_runs).find(|run| &run.harness != harness) {
            return Err(format!(
                "{}: the sides ran different harness builds ({} and {harness}), so there are no verdicts",
                scenario.name, run.harness
            ));
        }
        let kept = samples.entry(scenario.name.to_owned()).or_default();
        let environment = &base_runs[0].environment;
        kept.retain(|run| &run.harness == harness && &run.environment == environment);
        let mut base_counts = kept.clone();
        base_counts.extend(base_runs.iter().cloned());
        while base_counts.len() < SPREAD_RUNS {
            eprintln!("mln-bench: {}: another base run, for the spread", scenario.name);
            base_counts.push(run_in_process(&base, scenario.name, None)?);
        }
        let fresh = base_counts.len() - kept.len();
        kept.extend(base_counts[base_counts.len() - fresh..].iter().cloned());
        let excess = kept.len().saturating_sub(KEPT_RUNS);
        kept.drain(..excess);
        let probed: Vec<Run> = samples
            .get(&probe_key(scenario))
            .into_iter()
            .flatten()
            .filter(|run| &run.harness == harness && &run.environment == environment)
            .cloned()
            .collect();
        write_samples(&samples_path, &samples)?;
        let base = Base { runs: &base_runs, counts: &base_counts, probe: &probed };
        let mut comparison = compare_scenario(scenario, &base, &candidate_runs, options);
        if options.rounds.is_none() && comparison.has_count_verdict() {
            while candidate_runs.len() < CONFIRM_RUNS {
                eprintln!("mln-bench: {}: this checkout again, to confirm its verdicts", scenario.name);
                let run = run_in_process(&candidate, scenario.name, None)?;
                if &run.harness != harness {
                    return Err(format!("{}: this checkout's harness changed while it ran", scenario.name));
                }
                candidate_runs.push(run);
            }
            comparison = compare_scenario(scenario, &base, &candidate_runs, options);
        }
        comparisons.push(comparison);
    }
    if options.rounds.is_some() {
        judge_times(&mut comparisons, options);
    }
    Ok(Comparison {
        base: options.base.clone(),
        base_commit: commit,
        harness: env!("MLN_BENCH_HARNESS").to_owned(),
        rounds: options.rounds,
        threshold: options.threshold,
        instructions_threshold: options.instructions_threshold,
        time_threshold: options.time_threshold,
        scenarios: comparisons,
    })
}

/// The fewest rounds with which a time can get a verdict when `tests`
/// scenarios each test one: Holm's adjustment multiplies the smallest
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

/// Gives the tested time of each scenario of `comparisons` its p-value
/// Holm-adjusted over all of them, so that the chance of any false verdict
/// in the comparison stays under [`ALPHA`], and its verdict.
fn judge_times(comparisons: &mut [ScenarioComparison], options: &Options) {
    let mut tested: Vec<&mut MetricComparison> = comparisons
        .iter_mut()
        .filter(|scenario| scenario.no_verdicts.is_none())
        .flat_map(|scenario| scenario.metrics.iter_mut())
        .filter(|metric| metric.kind == MetricKind::Time && metric.p.is_some())
        .collect();
    let p_values: Vec<f64> = tested.iter().map(|metric| metric.p.unwrap_or(1.0)).collect();
    for (metric, p) in tested.iter_mut().zip(holm(&p_values)) {
        metric.p = Some(p);
        if let (Some(b), Some(c), Some(change)) = (metric.base, metric.candidate, metric.change)
            && p < ALPHA
            && change.abs() > options.time_threshold / 100.0
        {
            metric.verdict = Some(if c > b { Verdict::Worse } else { Verdict::Better });
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
    /// The layout probe's, for how far instructions move with where the
    /// code lies.
    probe: &'a [Run],
}

/// The comparison of one scenario's runs: `base`'s and `candidate_runs`,
/// this comparison's. With rounds, the tested time gets its p-value
/// unadjusted, and [`judge_times`] judges it.
fn compare_scenario(
    scenario: &Scenario,
    base_side: &Base<'_>,
    candidate_runs: &[Run],
    options: &Options,
) -> ScenarioComparison {
    let (base_runs, base_counts) = (base_side.runs, base_side.counts);
    let environment = candidate_runs[0].environment.clone();
    let mut no_verdicts = None;
    if let Some(run) = base_runs.iter().chain(candidate_runs).find(|run| run.environment != environment) {
        no_verdicts = Some(format!(
            "the sides ran in different environments ({})",
            environment.differences(&run.environment).join("; ")
        ));
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
            spread: None,
            p: None,
            verdict: None,
        };
        if let (Some(b), Some(c)) = (base_median, candidate_median)
            && kind == MetricKind::Count
        {
            let mut spread = range(&base).max(range(&candidate));
            let probe = values(base_side.probe, name);
            if name.starts_with("instructions") && !probe.is_empty() {
                spread = spread.max((median(&probe) - b).abs());
            }
            comparison.spread = Some(relative(spread, b));
            let threshold =
                if name.starts_with("instructions") { options.instructions_threshold } else { options.threshold };
            let noise = (threshold / 100.0 * b.abs()).max(SPREAD_FACTOR * spread);
            if (c - b).abs() > noise {
                comparison.verdict = Some(if c > b { Verdict::Worse } else { Verdict::Better });
            }
        }
        if options.rounds.is_some() && kind == MetricKind::Time && name == TESTED_TIME {
            comparison.p = (!base.is_empty() && !candidate.is_empty()).then(|| mann_whitney(&base, &candidate));
        }
        metrics.push(comparison);
    }
    if no_verdicts.is_some() {
        for metric in &mut metrics {
            metric.verdict = None;
        }
    }
    ScenarioComparison {
        scenario: scenario.name.to_owned(),
        environment,
        no_verdicts,
        base_runs: base_counts.len(),
        candidate_runs: candidate_runs.len(),
        metrics,
    }
}

impl ScenarioComparison {
    fn has_count_verdict(&self) -> bool {
        self.metrics.iter().any(|metric| metric.kind == MetricKind::Count && metric.verdict.is_some())
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
    let _ = writeln!(out, "harness {} on both sides", comparison.harness);
    let _ = writeln!(
        out,
        "counts: a verdict needs a change over {}% ({}% for instructions) and over {SPREAD_FACTOR}x the larger \
         spread of either side's runs; per-unit counts are context",
        comparison.threshold, comparison.instructions_threshold
    );
    match comparison.rounds {
        Some(rounds) => {
            let _ = writeln!(
                out,
                "times: {rounds} interleaved rounds; {TESTED_TIME} gets a verdict when its Mann-Whitney p, \
                 Holm-adjusted over the scenarios, is under {ALPHA} and its median moves over {}%; other times are context",
                comparison.time_threshold
            );
        }
        None => {
            let _ = writeln!(out, "times: context only, from one run per side (--rounds <n> tests them)");
        }
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
        if let Some(reason) = &scenario.no_verdicts {
            let _ = writeln!(out, "  no verdicts: {reason}");
        }
        let mut rows = vec![["metric", "base", "this", "change", "spread", "verdict"].map(str::to_owned).to_vec()];
        for metric in &scenario.metrics {
            let value =
                |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format_value(metric.unit, value));
            let spread = match (metric.spread, metric.p) {
                (Some(spread), _) => percent(spread, false),
                (None, Some(p)) => format!("p={p:.3}"),
                (None, None) => String::new(),
            };
            let verdict = match metric.verdict {
                Some(Verdict::Better) => "better",
                Some(Verdict::Worse) => "WORSE",
                None if metric.kind == MetricKind::PerUnit
                    || (metric.kind == MetricKind::Time && metric.p.is_none()) =>
                {
                    "(context)"
                }
                None => "",
            };
            rows.push(vec![
                metric.name.clone(),
                value(metric.base),
                value(metric.candidate),
                metric.change.map_or_else(String::new, |change| percent(change, true)),
                spread,
                verdict.to_owned(),
            ]);
        }
        out.push_str(&table(&rows));
    }
    let verdicts = comparison.verdicts();
    let _ = writeln!(out, "\n{verdicts} verdict{}", if verdicts == 1 { "" } else { "s" });
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
        command.arg("--target-dir").arg(dir);
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
/// and lays this checkout's harness over it: `tools/bench`, as a member of
/// its workspace, and each library's `bench` module, in place of the
/// commit's. A file is written only where its contents differ, so that
/// Cargo builds again only what changed since the last comparison.
fn prepare_tree(root: &Path, tree: &Path, commit: &str) -> Result<(), String> {
    if tree.join(".git").exists() {
        // The tree keeps the harness of the last comparison when it has the
        // commit already.
        if git(tree, &["rev-parse", "HEAD"]).ok().as_deref() != Some(commit) {
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
    sync_dir(&root.join("tools/bench"), &tree.join("tools/bench"))?;
    let modules = bench_modules(root)?;
    for module in &modules {
        write_if_changed(&tree.join(module), &read(&root.join(module))?)?;
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
                count("allocations_per_tile", MetricKind::PerUnit, allocations / 10.0),
                Metric { name: "wall".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
                Metric { name: "cpu".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
            ],
        }
    }

    fn options(rounds: Option<usize>) -> Options {
        Options {
            base: "main".to_owned(),
            filters: Vec::new(),
            rounds,
            threshold: 0.25,
            instructions_threshold: 1.0,
            time_threshold: 2.0,
        }
    }

    const SCENARIO: Scenario = Scenario { name: "micro/test/case", kind: Kind::Micro, question: "?", run: |_| {} };

    fn verdict(comparison: &ScenarioComparison, name: &str) -> Option<Verdict> {
        comparison.metrics.iter().find(|metric| metric.name == name).unwrap().verdict
    }

    #[test]
    fn a_count_needs_the_threshold_and_twice_the_spread() {
        let base = [run(1000.0, 1.0), run(1001.0, 1.0), run(1002.0, 1.0)];
        // +2%: over 1% and over twice the spread of 2.
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[] },
            &[run(1021.0, 5.0)],
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), Some(Verdict::Worse));
        // Times are context without rounds.
        assert_eq!(verdict(&comparison, "wall"), None);
        // +0.5%: under the threshold.
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[] },
            &[run(1006.0, 1.0)],
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), None);
        // A spread of 30 hides a change of 50.
        let noisy = [run(1000.0, 1.0), run(1015.0, 1.0), run(1030.0, 1.0)];
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &noisy[..1], counts: &noisy, probe: &[] },
            &[run(1065.0, 1.0)],
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), None);
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &noisy[..1], counts: &noisy, probe: &[] },
            &[run(900.0, 1.0)],
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), Some(Verdict::Better));
    }

    #[test]
    fn instructions_take_the_layout_probe_as_spread() {
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        // +2%, where moving the ref's code moved the count by 3%.
        let candidate = [run(1020.0, 1.0)];
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[run(1030.0, 1.0)] },
            &candidate,
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), None);
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[] },
            &candidate,
            &options(None),
        );
        assert_eq!(verdict(&comparison, "instructions"), Some(Verdict::Worse));
    }

    #[test]
    fn counts_that_repeat_take_a_smaller_threshold_than_instructions() {
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        // +0.5% of both: allocations get a verdict, instructions don't, and
        // per-unit counts are context.
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[] },
            &[run_with(1005.0, 1005.0, 1.0)],
            &options(None),
        );
        assert_eq!(verdict(&comparison, "allocations"), Some(Verdict::Worse));
        assert_eq!(verdict(&comparison, "instructions"), None);
        assert_eq!(verdict(&comparison, "allocations_per_tile"), None);
        assert!(comparison.has_count_verdict());
    }

    #[test]
    fn a_time_needs_rounds_and_a_significant_change() {
        let base: Vec<Run> = (0..6).map(|i| run(1000.0, 10.0 + f64::from(i) * 0.01)).collect();
        let slower: Vec<Run> = (0..6).map(|i| run(1000.0, 11.0 + f64::from(i) * 0.01)).collect();
        let same: Vec<Run> = (0..6).map(|i| run(1000.0, 10.005 + f64::from(i) * 0.01)).collect();
        let options = options(Some(6));
        let mut comparisons = vec![
            compare_scenario(&SCENARIO, &Base { runs: &base, counts: &base, probe: &[] }, &slower, &options),
            compare_scenario(&SCENARIO, &Base { runs: &base, counts: &base, probe: &[] }, &same, &options),
        ];
        judge_times(&mut comparisons, &options);
        assert_eq!(verdict(&comparisons[0], "wall"), Some(Verdict::Worse));
        // Only wall is tested.
        assert_eq!(verdict(&comparisons[0], "cpu"), None);
        assert_eq!(verdict(&comparisons[1], "wall"), None);
    }

    #[test]
    fn rounds_grow_with_the_scenarios_tested() {
        // 2 / C(8, 4) = 0.029 for one scenario; 2 / C(14, 7) x 50 = 0.029.
        assert_eq!(min_rounds(1), 4);
        assert_eq!(min_rounds(2), 5);
        assert_eq!(min_rounds(50), 7);
    }

    #[test]
    fn different_environments_get_no_verdicts() {
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut other = run(2000.0, 1.0);
        other.environment.cpu = "another".to_owned();
        let comparison = compare_scenario(
            &SCENARIO,
            &Base { runs: &base[..1], counts: &base, probe: &[] },
            &[other],
            &options(None),
        );
        assert!(comparison.no_verdicts.is_some());
        assert_eq!(verdict(&comparison, "instructions"), None);
    }

    #[test]
    fn adds_the_harness_to_the_workspace_once() {
        let manifest = "[workspace]\nmembers = [\n  \"crates/*\",\n]\n";
        let with = with_member(manifest).unwrap();
        assert!(with.contains("\"tools/bench\""));
        assert_eq!(with_member(&with).unwrap(), with);
    }
}
