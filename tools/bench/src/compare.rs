//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref (README.md).
//!
//! The ref is checked out in a worktree under `target/bench-compare`, which
//! the next comparison reuses, with this checkout's `tools/bench` laid over
//! it, so both sides run the same cases. When the harness doesn't build
//! there, because a case uses API the ref lacks, the ref runs its own
//! harness instead, and the cases only this checkout has are reported as
//! such. Each round runs one `mln-bench run`
//! process per side, in ABBA order so that drift hits both alike; a
//! process's value of a metric is one sample, since iterations in one
//! process share its caches and heap. A lead metric gets a verdict when a
//! two-sided Mann-Whitney U test, Holm-corrected over the case's lead
//! metrics, gives p < 0.05 and the medians differ by at least the
//! threshold. The first round keeps each side's last frames, and the report
//! says how many pixels differ, so a side that got faster by drawing less
//! shows up.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::stats::{holm, mann_whitney, median};
use crate::{Metrics, Report};

pub struct Options {
    pub base: String,
    /// Processes per side; even, for ABBA.
    pub rounds: usize,
    /// The smallest change of a median, in percent, that gets a verdict.
    pub threshold: f64,
    /// Arguments both sides' `mln-bench run` get: filters and scale.
    pub run_args: Vec<String>,
    pub fixtures: PathBuf,
    /// Also report the metrics that get no verdict.
    pub verbose: bool,
}

const ALPHA: f64 = 0.05;

/// Runs the comparison and returns its Markdown report.
pub fn compare(options: &Options) -> Result<String, String> {
    if options.rounds < 2 || options.rounds % 2 != 0 {
        return Err("--rounds must be even, and at least 2".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| e.to_string())?;
    let work = root.join("target/bench-compare");
    let commit = git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", options.base)])?;
    let (base_bin, own_harness) = build_base(&root, &work, &commit)?;
    let candidate_bin = std::env::current_exe().map_err(|e| e.to_string())?;

    let frames = work.join("frames");
    let _ = std::fs::remove_dir_all(&frames);
    let load_before = load_average();
    let mut runs: BTreeMap<Side, Vec<Report>> = BTreeMap::new();
    for round in 0..options.rounds {
        let order = if round % 4 == 0 || round % 4 == 3 {
            [Side::Base, Side::Candidate]
        } else {
            [Side::Candidate, Side::Base]
        };
        for side in order {
            eprintln!("mln-bench: round {} of {}: {}", round + 1, options.rounds, side.name());
            let bin = if side == Side::Base { &base_bin } else { &candidate_bin };
            let frames = (round == 0).then(|| frames.join(side.name()));
            runs.entry(side).or_default().push(run(bin, options, frames.as_deref())?);
        }
    }
    let load_after = load_average();

    let mut out = String::new();
    let base_env = &runs[&Side::Base][0].environment;
    let candidate_env = &runs[&Side::Candidate][0].environment;
    let short = &commit[..commit.len().min(12)];
    let _ = writeln!(out, "# mln-bench compare\n");
    let _ =
        writeln!(out, "- base: `{}` ({short}); candidate: this checkout, uncommitted changes included", options.base);
    let _ = writeln!(out, "- GPU: {}", candidate_env.gpu);
    let _ = writeln!(
        out,
        "- {} processes per side, ABBA; a verdict needs Holm-adjusted Mann-Whitney p < {ALPHA} and a change of at least {}%",
        options.rounds, options.threshold
    );
    if own_harness {
        let _ = writeln!(
            out,
            "- the base doesn't build this checkout's harness, so it ran its own: a case that changed between the two harnesses compares different work"
        );
    }
    if let (Some(before), Some(after)) = (load_before, load_after) {
        let _ = writeln!(out, "- load average: {before} before, {after} after");
    }
    let comparable = base_env == candidate_env;
    if !comparable {
        let _ = writeln!(
            out,
            "\n**No verdicts: the sides ran on different setups.**\n- base: {base_env:?}\n- candidate: {candidate_env:?}"
        );
    }
    if candidate_env.software_gpu {
        let _ = writeln!(out, "\n**A software rasterizer measures something else than a GPU.**");
    }
    let _ = writeln!(out);
    write_table(&mut out, &runs[&Side::Base], &runs[&Side::Candidate], options, comparable);
    let _ = writeln!(out);
    write_frames(&mut out, &frames);
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Base,
    Candidate,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Base => "base",
            Side::Candidate => "candidate",
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Checks `commit` out under `work`, lays this checkout's harness over it,
/// and builds it there; when that fails, builds the commit's own harness.
/// Returns the binary, and whether it is the commit's own harness.
fn build_base(root: &Path, work: &Path, commit: &str) -> Result<(PathBuf, bool), String> {
    let tree = work.join("tree");
    if tree.join(".git").exists() {
        git(&tree, &["checkout", "-q", "--force", "--detach", commit])?;
    } else {
        git(root, &["worktree", "prune"])?;
        let tree = tree.to_str().ok_or("a non-UTF-8 path")?;
        git(root, &["worktree", "add", "-q", "--detach", tree, commit])?;
    }
    let manifest = std::fs::read_to_string(tree.join("Cargo.toml")).map_err(|e| e.to_string())?;
    if !manifest.contains("\"tools/bench\"") {
        return Err(format!(
            "{commit} has no tools/bench in its workspace; compare against a commit that has the harness"
        ));
    }
    let harness = tree.join("tools/bench");
    std::fs::remove_dir_all(&harness).map_err(|e| e.to_string())?;
    copy_dir(&root.join("tools/bench"), &harness)?;
    let current = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let bench = std::fs::read_to_string(harness.join("Cargo.toml")).map_err(|e| e.to_string())?;
    std::fs::write(tree.join("Cargo.toml"), with_harness_dependencies(&manifest, &current, &bench))
        .map_err(|e| e.to_string())?;

    eprintln!("mln-bench: building the base ({commit})");
    let bin = work.join("target/release/mln-bench");
    if build(&tree, work)? {
        return Ok((bin, false));
    }
    eprintln!("mln-bench: the base doesn't build this checkout's harness; building its own");
    git(&tree, &["checkout", "-q", "--force", "HEAD", "--", "."])?;
    git(&tree, &["clean", "-q", "-d", "--force", "--", "tools/bench"])?;
    if !build(&tree, work)? {
        return Err("building the base failed, with this checkout's harness and with its own".into());
    }
    Ok((bin, true))
}

/// Builds the harness of the workspace at `tree`; whether it built.
fn build(tree: &Path, work: &Path) -> Result<bool, String> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "mln-rs-bench", "--bin", "mln-bench", "--manifest-path"])
        .arg(tree.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(work.join("target"))
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    Ok(status.success())
}

/// The ref's root manifest, with this checkout's entry of each workspace
/// dependency the harness inherits and the ref lacks, so that the harness
/// can gain dependencies and still build over older commits.
fn with_harness_dependencies(manifest: &str, current: &str, bench: &str) -> String {
    let entry = |text: &str, name: &str| {
        text.lines().find(|line| line.split_once(" = ").is_some_and(|(key, _)| key == name)).map(str::to_string)
    };
    let missing: Vec<String> = bench
        .lines()
        .filter_map(|line| line.strip_suffix(".workspace = true"))
        .filter(|name| entry(manifest, name).is_none())
        .filter_map(|name| entry(current, name))
        .collect();
    if missing.is_empty() {
        return manifest.to_string();
    }
    let header = "[workspace.dependencies]\n";
    manifest.replacen(header, &format!("{header}{}\n", missing.join("\n")), 1)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn run(bin: &Path, options: &Options, frames: Option<&Path>) -> Result<Report, String> {
    let mut command = Command::new(bin);
    command.args(["run", "--json", "--fixtures"]).arg(&options.fixtures).args(&options.run_args);
    if let Some(frames) = frames {
        command.arg("--frames").arg(frames);
    }
    let output = command.stderr(Stdio::inherit()).output().map_err(|e| format!("{}: {e}", bin.display()))?;
    if !output.status.success() {
        return Err(format!("{} failed ({})", bin.display(), output.status));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("{}'s report: {e}", bin.display()))
}

/// The 1-minute load average, where /proc has it.
fn load_average() -> Option<String> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next().map(str::to_string)
}

/// Each run's value of each metric of `case`, by metric, in the order of
/// the first run.
fn samples(reports: &[Report], case: &str) -> Vec<(String, bool, Vec<f64>)> {
    let metrics: Vec<&Metrics> =
        reports.iter().filter_map(|report| report.cases.iter().find(|c| c.name == case)).map(|c| &c.metrics).collect();
    let Some(first) = metrics.first() else {
        return Vec::new();
    };
    first
        .0
        .iter()
        .map(|metric| {
            let values = metrics.iter().filter_map(|m| m.get(&metric.name)).map(|m| m.value).collect();
            (metric.name.clone(), metric.lead, values)
        })
        .collect()
}

fn write_table(out: &mut String, base: &[Report], candidate: &[Report], options: &Options, comparable: bool) {
    let _ = writeln!(out, "| case | metric | base | candidate | change | p | verdict |");
    let _ = writeln!(out, "|---|---|--:|--:|--:|--:|---|");
    let (mut better, mut worse) = (0, 0);
    for case in candidate[0].cases.iter().map(|c| c.name.as_str()) {
        let base_samples: BTreeMap<String, Vec<f64>> =
            samples(base, case).into_iter().map(|(name, _, values)| (name, values)).collect();
        let rows: Vec<_> = samples(candidate, case)
            .into_iter()
            .filter_map(|(name, lead, values)| Some((base_samples.get(&name)?.clone(), name, lead, values)))
            .collect();
        if rows.is_empty() {
            let _ = writeln!(out, "| {case} | only in the candidate | | | | | |");
            continue;
        }
        let lead_p: Vec<f64> = rows.iter().filter(|row| row.2).map(|(b, _, _, c)| mann_whitney(b, c)).collect();
        let mut adjusted = holm(&lead_p).into_iter();
        for (base_values, name, lead, candidate_values) in rows {
            let (b, c) = (median(&base_values), median(&candidate_values));
            let change = if b == 0.0 { if c == 0.0 { 0.0 } else { f64::INFINITY } } else { 100.0 * (c - b) / b };
            let p = if lead { adjusted.next() } else { None };
            if !lead && !options.verbose {
                continue;
            }
            let verdict = match p {
                Some(p) if comparable && p < ALPHA && change.abs() >= options.threshold => {
                    if change < 0.0 {
                        better += 1;
                        "better"
                    } else {
                        worse += 1;
                        "**worse**"
                    }
                }
                _ => "",
            };
            let p = p.map(|p| format!("{p:.3}")).unwrap_or_default();
            let _ =
                writeln!(out, "| {case} | {name} | {} | {} | {change:+.1}% | {p} | {verdict} |", number(b), number(c));
        }
    }
    let _ = writeln!(out, "\n{better} better, {worse} worse.");
}

fn number(value: f64) -> String {
    match value.abs() {
        v if v >= 100.0 => format!("{value:.0}"),
        v if v >= 10.0 => format!("{value:.1}"),
        _ => format!("{value:.3}"),
    }
}

/// How many pixels of each last frame differ between the sides, by the
/// render tests' pixelmatch threshold; the differences go to `diff/`.
fn write_frames(out: &mut String, frames: &Path) {
    let (base, candidate, diff) = (frames.join("base"), frames.join("candidate"), frames.join("diff"));
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".png"))
        .collect();
    names.sort();
    let _ = writeln!(out, "Last frames ({}):\n", diff.display());
    for name in names {
        let read = |dir: &Path| {
            let bytes = std::fs::read(dir.join(&name)).map_err(|e| e.to_string())?;
            mln_image::decode_png(&bytes)
        };
        let line = match (read(&base), read(&candidate)) {
            (Ok(a), Ok(b)) if a.size == b.size => {
                let (width, height) = (a.size.width as usize, a.size.height as usize);
                let mut output = vec![0u8; a.data.len()];
                let different =
                    pixelmatch::pixelmatch(&a.data, &b.data, width, height, Some(&mut output), 0.1285, false);
                if different > 0 {
                    let image = mln_util::image::PremultipliedImage::from_vec(a.size, output);
                    crate::scene::write_frame(&diff, name.trim_end_matches(".png"), &image);
                }
                format!("{different} of {} pixels differ", width * height)
            }
            (Ok(_), Ok(_)) => "the sizes differ".to_string(),
            (a, b) => [a.err(), b.err()].into_iter().flatten().collect::<Vec<_>>().join("; "),
        };
        let _ = writeln!(out, "- {name}: {line}");
    }
}
