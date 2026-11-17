# mln-bench

The benchmarks of mln-rs, for evaluating a code change: does it make the work
mln-rs does for a map larger or smaller, and by how much. One binary,
`mln-bench`, holds every scenario.

- A **micro** scenario runs one pipeline stage as a function over data, with no
  map, GPU or threads: `micro/<stage>/<case>`.
- A **macro** scenario runs a map through something a user waits for or feels,
  such as loading or drawing frames: `macro/<operation>/<workload>`.

A scenario whose cost grows with its input runs at two sizes, named by a last
segment (`micro/geojson/parse/small`, `.../large`), so that a change in how its
cost grows shows as a changed ratio between the two.

Each scenario does the same work on every run and every machine: fixed frame
counts, cameras chosen by frame index, data generated from a fixed seed, and no
waiting on the wall clock. Each runs in a process of its own, so no scenario
inherits another's heap, threads or caches.

## Running

```sh
mise run bench                         # every scenario
mise run bench -- --filter micro/      # those whose names contain micro/
mise run bench:list                    # the scenarios and their questions
mise run bench:compare main            # this checkout against main
```

`mise run bench` builds with the `benchmark` profile (`profile.toml`) and runs
`mln-bench run`; `--json` prints the runs as JSON. The headless frontend draws
on WebGPU unless `MLN_RENDER_BACKEND` names another backend (`metal`, `vulkan`,
`d3d12`, `gl`).

## Metrics

Every metric is a **count** or a **time**, and lower is better for all of them.

Every scenario reports these, over the parts of its run it measures (its setup
is left out):

| metric          | kind  | what                                                                    |
| --------------- | ----- | ----------------------------------------------------------------------- |
| `instructions`  | count | retired by every thread of the process (macOS and Linux only)           |
| `allocations`   | count | heap allocations of every thread                                        |
| `allocated`     | count | the bytes those allocations asked for                                   |
| `peak_heap`     | count | the most heap bytes live at once, above those live when measuring began |
| `retained_heap` | count | the heap bytes live at the end, above those live when measuring began   |
| `wall`, `cpu`   | time  | wall-clock time, and the CPU time of every thread of the process        |

A scenario adds its own, such as `frames`, `frames_incomplete` (frames drawn
with tiles missing), `gpu_buffers`, or frame-time percentiles (`frame_p50`,
`frame_p90`, `frame_p99`).

Counts repeat closely from run to run, so one run of each side shows a change.
Times depend on the machine's load and are context unless a comparison runs
rounds.

## Comparing

`mln-bench compare <ref>` checks the ref out in a Git worktree under
`target/bench-compare/tree`, lays this checkout's `tools/bench` over it, and
builds it there, so both sides run this checkout's scenarios on their own
libraries. Both build with this checkout's toolchain and `profile.toml`, and no
path of either checkout ends up in a binary, so the same source builds the same
binary on both sides. Each run reports its harness id, a hash of the harness's
files and the compiler, and the comparison refuses runs whose ids differ.

When this checkout's harness doesn't build against the ref, the comparison stops
with no verdicts. It never runs the ref's own harness instead. This happens when
a change adds a scenario that needs a library hook the ref lacks, or changes the
library API a scenario calls. Commit the hook or the API change with the
scenario first, then compare the change you want to measure with that commit
(`mise run bench:compare HEAD~1`). Keeping the code that scenarios call behind
stable functions keeps this rare.

### Counts

Each side runs once per scenario by default. The ref's runs are kept in
`target/bench-compare/samples`, by commit, and the ref runs again only until it
has three: the first comparison with a commit costs two extra runs per scenario,
later ones none. A count gets a verdict when this checkout's value moves from
the ref's median by more than the threshold (`--threshold`, 1% by default) and
by more than twice the count's spread, the larger range of either side's runs. A
count that varies between runs needs a larger change to get a verdict.

### Times

Without `--rounds`, times show what one run of each side took, as context.
`--rounds <n>` (at least 4) runs each side n times, interleaved ABBA so that
changes in the machine's load hit both alike. A time then gets a verdict when a
two-sided Mann-Whitney U test, Holm-adjusted over the scenario's times, gives p
< 0.05 and the median moves by more than `--time-threshold` (2% by default).

### Environments

Each run records its environment: the OS, the CPU, how it counted instructions,
the GPU it drew on and its backend, whether that GPU is a CPU rasterizer such as
lavapipe, and the fixtures it read. A scenario whose sides ran in different
environments gets no verdicts, so a CPU rasterizer's numbers are compared only
with a CPU rasterizer's.

## Reading a report

```text
micro/expression/evaluate (counts: 3 base runs, 1 of this checkout)
  metric             base      this  change  spread    verdict
  instructions     1.13 G    1.15 G   +2.2%   0.16%      WORSE
  allocations       664 k     676 k   +1.8%   0.00%      WORSE
  wall            56.8 ms   54.9 ms   -3.3%          (context)
```

`base` is the ref's median, `this` this checkout's, `change` the difference as a
share of the base. `spread` is a count's run-to-run range as a share of the
base, or with rounds a time's adjusted p-value. `WORSE` and `better` are
verdicts; a blank is no verdict, and `(context)` marks a time without rounds.
The last line counts the verdicts; comparing a commit with itself gives none.

## Adding a scenario

A scenario answers one question about one stage or operation. Add it to the
module of its stage (`src/scenarios/micro/`) or operation
(`src/scenarios/macro/`), and its module's `SCENARIOS` to `scenarios::all`:

```rust
pub(crate) const SCENARIOS: &[Scenario] = &[Scenario {
    name: "micro/expression/evaluate",
    kind: Kind::Micro,
    question: "What does evaluating a style's kinds of expressions over features cost?",
    run: evaluate,
}];

fn evaluate(bench: &mut Bench) {
    let (expressions, features) = setup(); // not measured
    bench.measure(|| evaluate_all(&expressions, &features));
    bench.count("evaluations", (features.len() * expressions.len()) as u64);
}
```

`Bench::measure` runs the part that counts; call it once, or once per frame to
leave waits between frames out. `count`, `bytes`, `time` and `percentiles`
declare the scenario's own metrics. A macro scenario sets `bench.gpu` to the
device it drew on. What more than one scenario uses goes in `src/support/`: the
headless map (`support::map`) and seeded data (`support::random`).
