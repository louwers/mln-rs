# mln-bench

The benchmarks of mln-rs, for evaluating a code change: does it make the work
mln-rs does for a map larger or smaller, and by how much. One binary,
`mln-bench`, holds every scenario.

- A **micro** scenario runs one pipeline stage as a function over data, with no
  map, GPU or threads: `micro/<stage>/<case>`.
- A **macro** scenario runs a map through something a user waits for or feels,
  such as loading or drawing frames: `macro/<operation>/<workload>`.

A scenario whose cost grows with its input runs at two sizes, named by a last
segment (`micro/geojson/parse/small` and `.../large`, or `sparse` and `dense`
for a tile), so that a change in how its cost grows shows as a changed ratio
between the two. A scenario that repeats a unit of work reports its counts per
unit too, which are alike at both sizes while the cost grows in proportion.

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
mise run bench:fixtures                # install the map data, once
```

`mise run bench` builds with the `benchmark` profile (`profile.toml`) and runs
`mln-bench run`, after `mise run bench:fixtures`; `--json` prints the runs as
JSON, and `--images <dir>` writes the last image each macro scenario drew to a
PNG file there. Macro scenarios draw on the device the tests draw on: WebGPU,
unless `MLN_RENDER_BACKEND` names another backend (`metal`, `vulkan`, `d3d12`,
`gl`).

## Scenarios

| scenario                             | question                                                                                                | one unit                                                                              |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `micro/style/parse/small`            | What does parsing a style document of 110 layers cost?                                                  | a parse of Liberty                                                                    |
| `micro/style/parse/large`            | What does parsing a style document of 440 layers cost?                                                  | a parse of Liberty with its layers four times over                                    |
| `micro/style/edit/small`             | What do an application's edits to a style of 110 layers cost, each compared with the version before?    | an edit, and the comparison of the two versions                                       |
| `micro/style/edit/large`             | What do an application's edits to a style of 440 layers cost, each compared with the version before?    | an edit                                                                               |
| `micro/expression/parse/liberty`     | What does parsing a style's filters and property expressions cost?                                      | one of Liberty's 234 expressions, parsed into its property's type                     |
| `micro/expression/evaluate/liberty`  | What does evaluating a style's filters and property expressions over a tile's features cost?            | an expression evaluated on a feature                                                  |
| `micro/decode/mvt/sparse`            | What does decoding a sparse MVT tile cost, its features selected by a style's filters?                  | a tile of 186 features                                                                |
| `micro/decode/mvt/dense`             | What does decoding a dense MVT tile cost, its features selected by a style's filters?                   | a tile of 6,919 features                                                              |
| `micro/decode/mlt/sparse`            | What does decoding a sparse MLT tile cost, its features selected by a style's filters?                  | a tile                                                                                |
| `micro/decode/mlt/dense`             | What does decoding a dense MLT tile cost, its features selected by a style's filters?                   | a tile                                                                                |
| `micro/geojson/parse/small`          | What does parsing GeoJSON text of 1.5 MB cost?                                                          | a KB of text: 33 countries' borders                                                   |
| `micro/geojson/parse/large`          | What does parsing GeoJSON text of 13 MB cost?                                                           | a KB of text: 258 countries' borders                                                  |
| `micro/geojson/cut/small`            | What does cutting the world's tiles of zooms 0 to 4 from 33 countries' borders cost?                    | a KB of the text the data is built from                                               |
| `micro/geojson/cut/large`            | What does cutting the world's tiles of zooms 0 to 4 from 258 countries' borders cost?                   | a KB of text                                                                          |
| `micro/layout/fill/sparse`           | What does laying out a sparse tile's fill layers cost?                                                  | a tile laid out by Liberty's 16 fill layers                                           |
| `micro/layout/fill/dense`            | What does laying out a dense tile's fill layers cost?                                                   | a tile                                                                                |
| `micro/layout/line/sparse`           | What does laying out a sparse tile's line layers cost?                                                  | a tile laid out by Liberty's 67 line layers                                           |
| `micro/layout/line/dense`            | What does laying out a dense tile's line layers cost?                                                   | a tile                                                                                |
| `micro/layout/fill-extrusion/sparse` | What does laying out a sparse tile's extruded buildings cost?                                           | a tile laid out by Liberty's fill-extrusion layer                                     |
| `micro/layout/fill-extrusion/dense`  | What does laying out a dense tile's extruded buildings cost?                                            | a tile                                                                                |
| `micro/layout/symbol/sparse`         | What does laying out a sparse tile's labels and icons cost?                                             | a tile laid out by Liberty's 25 symbol layers                                         |
| `micro/layout/symbol/dense`          | What does laying out a dense tile's labels and icons cost?                                              | a tile                                                                                |
| `macro/load/street`                  | What does showing a city's streets from nothing take, until the first complete frame?                   | a load: the map, renderer and workers start; the style, tiles, glyphs and sprite load |
| `macro/load/globe`                   | What does showing the whole world on the globe from nothing take, until the first complete frame?       | a load                                                                                |
| `macro/load/cjk`                     | What does showing a city labelled in Japanese script from nothing take, until the first complete frame? | a load                                                                                |
| `macro/frame/street`                 | What does a frame of a city's streets cost while the camera pans, turns and tilts over loaded tiles?    | a frame: the update, symbol placement, draw calls and the GPU finishing it            |
| `macro/frame/globe`                  | What does a frame of the globe cost while it turns and tilts over loaded tiles?                         | a frame                                                                               |
| `macro/navigate/street`              | What does a flight into a city's streets and back out cost, with tiles arriving while it flies?         | a frame, with the layout of what arrived for it                                       |
| `macro/update/geojson`               | What does replacing a GeoJSON source's data cost, until the map shows the new data?                     | an update of 2,000 points: parsed, indexed, tiled, laid out and shown                 |
| `macro/update/layers`                | What does adding a batch of layers to a city's map, then removing them, cost until the map shows each?  | a batch of 20 layers added or removed, and the tiles laid out again                   |
| `macro/update/paint`                 | What does setting a layer's paint property cost, until the map shows the new value?                     | an edit, and the frames of its transition                                             |
| `macro/update/feature-state`         | What does a frame cost that moves a hover state from one feature to another?                            | a frame that moves the hover state                                                    |
| `macro/update/images`                | What does adding the images that visible symbols use cost, until the map shows them?                    | 24 images added, and the tiles that use them laid out again                           |
| `macro/tile-server/street`           | What does a tile server's renderer spend on each new tile of a city, from zoom 7 to 14?                 | a tile: laid out, placed, drawn and read back by a reused renderer                    |

`mise run bench:list` prints the questions, and each scenario's doc comment says
what it runs.

The micro scenarios read what a map over the fixtures reads: Liberty's style
document, its expressions, and two tiles of zoom 14 from the street tiles, as
MVT and as MLT: the densest, in San Francisco's South of Market, and a sparse
one at Candlestick Point on the bay's shore, with every kind of feature but few
of them. The style and layout scenarios run on the style as Liberty is and with
its layers four times over, a style of many layers. The GeoJSON scenarios read
Natural Earth's countries, and every eighth of them for the small size. A layout
scenario lays the tile out by the style's layers of one type, as a tile worker's
job does once the tile's glyphs and images are resolved; it reports the tile's
buckets as its retained heap.

The load scenarios start a map over the fixtures (Liberty over the street, world
or Tokyo tiles) at 1024 x 768 and end at its first frame with nothing missing;
making the GPU device is left out. The frame scenarios load the start of their
path, then measure 120 frames along a camera path that keeps the same tiles in
view: the renderer loads a tile again when it comes back into view, which a
frame over loaded tiles mustn't do. The navigate scenario flies 240 frames from
zoom 11 into the streets at zoom 15.5 and back out to zoom 12, and each request
it makes is answered 6 frames later, as over a network, so its frames show tiles
missing and arriving. The update scenarios each edit a settled street map at
zoom 15 and run until it shows the edit, complete and with nothing left to
animate; the hover scenario moves a feature's state at each of 120 frames
instead. The tile server renders thirteen new 512-pixel tiles of zooms 7 to 14
as still images in tile mode, with one map and renderer that have the style
loaded.

## Fixtures

Scenarios read map data from a snapshot that never changes and never touches the
network: [`fixtures.json`](fixtures.json) names each file with its size and
SHA-256, where the snapshot is published, and the data's sources, licenses and
attribution. It holds:

- `tiles/street.pmtiles`: OpenFreeMap's vector tiles (OpenMapTiles schema) of
  central San Francisco, zooms 0 to 14; `tiles/street-mlt.pmtiles`, the same
  tiles as MLT.
- `tiles/world.pmtiles`: the whole world, zooms 0 to 4, for the globe.
- `tiles/cjk.pmtiles`: central Tokyo, zooms 0 to 14, its names in Japanese
  script.
- `styles/liberty.json`: OpenFreeMap's Liberty style, its URLs turned into
  `asset://` URLs of the snapshot (and any layer it can't draw from the snapshot
  removed, as the manifest's `style_changes` says), with its sprite and the
  glyph ranges the tiles' labels need.
- `fonts/`: Noto Sans and Noto Sans JP, for text scenarios that read a font.
- `geojson/ne_10m_admin_0_countries.geojson`: Natural Earth's countries.

Each tile set comes with its area in the manifest: a camera centered in it sees
only tiles of the set when its view is at most two tiles across, and a map
requests no tile outside the archive's bounds, so no frame waits for a tile the
set lacks.

`mise run bench:fixtures` installs the snapshot from its base URL in a cache
that every checkout of the user shares, `mln-rs/bench-fixtures/<manifest hash>`
in `~/Library/Caches` (macOS), `$XDG_CACHE_HOME` or `~/.cache` (Linux) or
`%LOCALAPPDATA%` (Windows); `MLN_BENCH_FIXTURES` names another directory. It
downloads into a temporary directory beside that one, checks every file, and
renames it into place, so a session sees the snapshot complete or not at all,
and nothing changes it afterwards. `--from <dir>` installs from a directory laid
out as the base URL is.

A scenario opens it with `support::fixtures::Fixtures::open(bench)`, which
records the manifest's hash in the run's environment, so runs on different
fixtures get no verdicts against each other. A scenario fails, naming the
command, when the snapshot isn't installed. Data an application would supply,
such as features to add, is generated in code from a fixed seed instead.

### A new snapshot

`mise run bench:snapshot <id>` (`tools/bench-snapshot`) fetches the pinned
OpenFreeMap planet build, the style, its sprite and glyphs, the font files and
the GeoJSON, at most 8 requests at once; writes the archives, the street tiles
as MLT and the style; checks that the tiles hold the style's layers and read
back from the archives; stages the snapshot in
`target/bench-fixtures-upload/<id>/`, laid out as it is published; and points
`fixtures.json` at it. Before publishing it, install it with
`mise run bench:fixtures --from target/bench-fixtures-upload/<id>` and run the
scenarios that read it. A published snapshot never changes: a new one gets a new
id.

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
with tiles missing), `tile_requests`, `instructions_largest_frame` (the
costliest single frame's), `gpu_buffers` (the bytes of the GPU's vertex and
index buffers after the last frame), `gpu_objects` (the textures and buffers the
renderer made), or frame-time percentiles (`frame_p50`, `frame_p90`,
`frame_p99`). A scenario that repeats a unit of work, such as a frame or a tile,
also reports `instructions`, `allocations` and `allocated` per unit
(`instructions_per_frame`).

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
micro/expression/evaluate/liberty (counts: 3 base runs, 1 of this checkout)
  metric             base      this  change  spread    verdict
  instructions     1.01 G    1.03 G   +2.2%   0.01%      WORSE
  allocations       687 k     699 k   +1.8%   0.00%      WORSE
  wall            39.4 ms   40.6 ms   +3.0%          (context)
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
    name: "micro/expression/evaluate/liberty",
    kind: Kind::Micro,
    question: "What does evaluating a style's filters and property expressions over a tile's features cost?",
    run: evaluate,
}];

fn evaluate(bench: &mut Bench) {
    let (expressions, features) = setup(); // not measured
    bench.measure(|| evaluate_all(&expressions, &features));
    bench.per("evaluation", (features.len() * expressions.len()) as u64);
}
```

`Bench::measure` runs the part that counts; call it once, or once per frame to
leave waits between frames out. `count`, `bytes`, `time` and `percentiles`
declare the scenario's own metrics, and `per` the unit of work it repeats. A
macro scenario sets `bench.gpu` to the device it drew on, and writes its last
image to `bench.image` when that is set. What more than one scenario uses goes
in `src/support/`: the headless map (`support::map`), the fixtures
(`support::fixtures`) and seeded data (`support::random`); what more than one
micro scenario uses, such as the tiles they read, in `src/scenarios/micro/`.

### Reaching a library's internal stages

A micro scenario calls a stage as a function over data. Where the stage is
internal to its library, the library has a `bench` feature, off by default, and
behind it a module `bench` that is hidden from the documentation:
`mln_renderer::bench::TileLayout` runs a tile's layout job, for instance. The
harness turns the feature on in its `Cargo.toml`, so builds without the harness
leave the module out. Each entry point takes and returns the library's public
types and nothing internal, so that a change to the stage's internals changes
the entry point, not the scenario, and the harness still builds against older
commits that have the entry point. A comparison with a commit from before an
entry point existed doesn't build; compare with a later one.

### Driving a map

`support::map::HeadlessMap` is a map on a headless frontend that runs only when
the scenario drives it, so that it does the same work on every run:

- Each frame is a tick of a 60 Hz frame clock that only frames advance, so
  symbol placement, fades and transitions don't depend on how long a frame took.
- The map's background work (file reads, tile layout, glyphs, the sprite) runs
  on the process's thread pool behind `support::workers::Workers`, which knows
  when none is left. `HeadlessMap::load`, `settle` and `render_still` wait for
  it before each frame, so each frame has everything the frames before it asked
  for, and a map loads in the same steps on every run.
- `support::map::Frontend` holds the GPU device and target, made apart from the
  map, so that a scenario can leave them out of what it measures.
- A GeoJSON source that `HeadlessMap::add_geojson_source` adds cuts its tiles on
  those workers too, rather than on threads of its own.
- `support::latency::Latency` holds a file source's answers back for a number of
  frames: a scenario that calls its `tick` before each frame, then waits for the
  workers, sees each answer arrive the same number of frames after its request
  on every run.

### Repeating counts

A count repeats only if the process that counts it does the same work in every
run, which takes two things of the runner:

- Where the system places a process's code, stack and heap at random, the
  instructions that copies, comparisons and searches take change with the
  addresses, from one process to the next. A scenario's process
  (`mln-bench exec`) therefore runs again at fixed addresses where the system
  lets it, macOS and Linux (`src/aslr.rs`).
- The first process of a newly built binary to run some code does more work than
  later ones: on macOS the system checks each page of the binary's code the
  first time a process reads it, and caches the GPU code it compiles for each
  binary. `run` and `compare` therefore run each scenario once on each binary
  before they measure, and drop what that run reports.
