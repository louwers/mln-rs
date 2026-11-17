# mln-bench

The benchmarks of mln-rs, for evaluating a code change: does it make the work
mln-rs does for a map larger or smaller, and by how much. One binary,
`mln-bench`, holds every scenario.

- A **micro** scenario runs one pipeline stage alone: `micro/<stage>/<case>`.
  Most are a function over data, with no map, GPU or threads; `micro/draw` draws
  one layer type alone, over loaded data, which takes a map and the GPU.
- A **macro** scenario runs a map through something a user waits for or feels,
  such as loading or drawing frames: `macro/<operation>/<workload>`.

Each scenario does the same work on every run and every machine: fixed frame
counts, cameras chosen by frame index, data generated from a fixed seed, and no
waiting on the wall clock. Each runs in a process of its own, so no scenario
inherits another's heap, threads or caches.

A third kind runs outside `mln-bench`: the desktop example's **presented**
benchmark (`mise run desktop --profile release -- --benchmark[=<path>]`, where
the path is `street`, `globe`, `antimeridian`, `earthquakes` or `terrain`) draws
the camera path of a `macro/frame` scenario in a window, over the same fixtures,
and prints the times between the frames the window presents. It shows what a
user sees of a frame, the backend's surface and the display's compositor
included, which a headless frame leaves out. Nothing runs it but a person at a
machine with a display: run it on both sides of a change by hand, and compare
the summaries. `--laps <n>` sets how long it runs (10 laps of 120 frames; 0
draws until the window closes, for a profiler). The fixtures and the paths come
from `workload/`, a crate both programs share.

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
PNG file there.

Macro scenarios draw on the device the tests draw on: WebGPU, unless
`MLN_RENDER_BACKEND` names another backend (`metal`, `vulkan`, `d3d12`, `gl`).
The device may be a GPU or a CPU rasterizer, such as lavapipe or llvmpipe on a
server without a GPU, or on a Linux machine whose Vulkan loader is given only
lavapipe's driver (`VK_DRIVER_FILES`). Each run records which, and a comparison
compares a configuration only with itself.

## Scenarios

Each scenario answers one question; its doc comment says what it runs and why
its sizes are the ones it uses.

| scenario                                | question                                                                                                                                   |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `micro/style/parse/small`               | What does parsing a style document of 110 layers cost?                                                                                     |
| `micro/style/parse/large`               | What does parsing a style document of 440 layers cost?                                                                                     |
| `micro/style/edit/small`                | What do an application's edits to a style of 110 layers cost, each compared with the version before?                                       |
| `micro/style/edit/large`                | What do an application's edits to a style of 440 layers cost, each compared with the version before?                                       |
| `micro/expression/parse/liberty`        | What does parsing a style's filters and property expressions cost?                                                                         |
| `micro/expression/evaluate/liberty`     | What does evaluating a style's filters and property expressions over a tile's features cost?                                               |
| `micro/decode/mvt/sparse`               | What does decoding a sparse MVT tile cost, its features selected by a style's filters?                                                     |
| `micro/decode/mvt/dense`                | What does decoding a dense MVT tile cost, its features selected by a style's filters?                                                      |
| `micro/decode/mlt/sparse`               | What does decoding a sparse MLT tile cost, its features selected by a style's filters?                                                     |
| `micro/decode/mlt/dense`                | What does decoding a dense MLT tile cost, its features selected by a style's filters?                                                      |
| `micro/geojson/parse/small`             | What does parsing GeoJSON text of 1.5 MB cost?                                                                                             |
| `micro/geojson/parse/large`             | What does parsing GeoJSON text of 13 MB cost?                                                                                              |
| `micro/geojson/cut/small`               | What does cutting the world's tiles of zooms 0 to 4 from 33 countries' borders cost?                                                       |
| `micro/geojson/cut/large`               | What does cutting the world's tiles of zooms 0 to 4 from 258 countries' borders cost?                                                      |
| `micro/text/glyph-ranges/small`         | What does parsing 7 glyph ranges, those of a tile's labels in Latin script, cost?                                                          |
| `micro/text/glyph-ranges/large`         | What does parsing 99 glyph ranges, those of a tile's labels in Japanese script, cost?                                                      |
| `micro/text/glyph-sdf/sparse`           | What does drawing a sparse tile's Japanese glyphs from a font, as distance fields, cost?                                                   |
| `micro/text/glyph-sdf/dense`            | What does drawing a dense tile's Japanese glyphs from a font, as distance fields, cost?                                                    |
| `micro/text/shape/sparse`               | What does shaping a sparse tile's labels into lines of glyphs cost?                                                                        |
| `micro/text/shape/dense`                | What does shaping a dense tile's labels into lines of glyphs cost?                                                                         |
| `micro/layout/fill/sparse`              | What does laying out a sparse tile's fill layers cost?                                                                                     |
| `micro/layout/fill/dense`               | What does laying out a dense tile's fill layers cost?                                                                                      |
| `micro/layout/line/sparse`              | What does laying out a sparse tile's line layers cost?                                                                                     |
| `micro/layout/line/dense`               | What does laying out a dense tile's line layers cost?                                                                                      |
| `micro/layout/fill-extrusion/sparse`    | What does laying out a sparse tile's extruded buildings cost?                                                                              |
| `micro/layout/fill-extrusion/dense`     | What does laying out a dense tile's extruded buildings cost?                                                                               |
| `micro/layout/symbol/sparse`            | What does laying out a sparse tile's labels and icons cost?                                                                                |
| `micro/layout/symbol/dense`             | What does laying out a dense tile's labels and icons cost?                                                                                 |
| `micro/symbols/index/sparse`            | What does indexing a sparse tile's symbols across zooms cost, as the view zooms in on them?                                                |
| `micro/symbols/index/dense`             | What does indexing a dense tile's symbols across zooms cost, as the view zooms in on them?                                                 |
| `micro/symbols/place/sparse`            | What does placing a sparse tile's labels and icons cost, each where it collides with none shown?                                           |
| `micro/symbols/place/dense`             | What does placing a dense tile's labels and icons cost, each where it collides with none shown?                                            |
| `micro/feature-state/apply/sparse`      | What does applying feature state to a sparse tile's roads and buildings cost?                                                              |
| `micro/feature-state/apply/dense`       | What does applying feature state to a dense tile's roads and buildings cost?                                                               |
| `micro/worker-codec/tile/sparse`        | What does a sparse tile's job and its laid-out reply cost to cross between the map and a worker instance?                                  |
| `micro/worker-codec/tile/dense`         | What does a dense tile's job and its laid-out reply cost to cross between the map and a worker instance?                                   |
| `micro/queue/commands/small`            | What does a command cost through a thread's queue and a turn of it, sent one per turn?                                                     |
| `micro/queue/commands/large`            | What does a command cost through a thread's queue and a turn of it, sent 256 per turn?                                                     |
| `micro/queue/timers/small`              | What do the deadlines of 64 requests in flight cost a thread's queue?                                                                      |
| `micro/queue/timers/large`              | What do the deadlines of 4,096 requests in flight cost a thread's queue?                                                                   |
| `micro/draw/fill/constant`              | What does drawing a city's buildings as fills of one colour cost?                                                                          |
| `micro/draw/fill/data-driven`           | What does drawing a city's buildings as fills coloured by their height cost?                                                               |
| `micro/draw/fill/pattern`               | What does drawing a city's buildings as fills of a sprite's pattern cost?                                                                  |
| `micro/draw/line/constant`              | What does drawing a city's roads as lines of one width and colour cost?                                                                    |
| `micro/draw/line/data-driven`           | What does drawing a city's roads as lines whose width and colour follow their class cost?                                                  |
| `micro/draw/line/dash`                  | What does drawing a city's roads as dashed lines cost?                                                                                     |
| `micro/draw/line/pattern`               | What does drawing a city's roads as lines of a sprite's pattern cost?                                                                      |
| `micro/draw/line/gradient`              | What does drawing a long route as a line coloured along its length cost?                                                                   |
| `micro/draw/fill-extrusion/constant`    | What does drawing a city's buildings as extrusions of one height and colour cost, seen tilted?                                             |
| `micro/draw/fill-extrusion/data-driven` | What does drawing a city's buildings as extrusions of their own height, coloured by it, cost, seen tilted?                                 |
| `micro/draw/circle/constant`            | What does drawing a month's earthquakes over the world as circles of one size and colour cost?                                             |
| `micro/draw/circle/data-driven`         | What does drawing a month's earthquakes over the world as circles sized by magnitude and coloured by depth cost?                           |
| `micro/draw/heatmap/constant`           | What does drawing a month's earthquakes over the world as a heatmap of equal weights cost?                                                 |
| `micro/draw/heatmap/data-driven`        | What does drawing a month's earthquakes over the world as a heatmap weighted by magnitude cost?                                            |
| `micro/draw/symbol/icon`                | What does drawing a city's points of interest as icons cost, those that collide hidden?                                                    |
| `micro/draw/symbol/text`                | What does drawing a city's points of interest as haloed labels cost, those that collide hidden?                                            |
| `micro/draw/raster/terrain`             | What does drawing a city's terrain tiles as raster images cost?                                                                            |
| `micro/draw/hillshade/terrain`          | What does hillshading a city's hills cost?                                                                                                 |
| `micro/draw/color-relief/terrain`       | What does tinting a city's hills by elevation cost?                                                                                        |
| `macro/load/street`                     | What does showing a city's streets from nothing take, until the first complete frame?                                                      |
| `macro/load/globe`                      | What does showing the whole world on the globe from nothing take, until the first complete frame?                                          |
| `macro/load/cjk`                        | What does showing a city labelled in Japanese script from nothing take, until the first complete frame?                                    |
| `macro/load/runtime`                    | What does showing a city's streets take through the runtime, from its start until a still image?                                           |
| `macro/frame/street`                    | What does a frame of a city's streets cost while the camera pans, turns and tilts over loaded tiles?                                       |
| `macro/frame/globe`                     | What does a frame of the globe cost while it turns and tilts over loaded tiles?                                                            |
| `macro/frame/antimeridian`              | What does a frame of the world on a flat map cost while the view crosses the antimeridian over loaded tiles?                               |
| `macro/frame/earthquakes`               | What does a frame of a map of earthquakes cost, a heatmap that gives way to circles, while the camera circles and zooms in and out?        |
| `macro/frame/terrain`                   | What does a frame of a city's hills cost, tinted by elevation and hillshaded, while the camera circles, turns and tilts over loaded tiles? |
| `macro/navigate/street`                 | What does a flight into a city's streets and back out cost, with tiles arriving while it flies?                                            |
| `macro/navigate/fly`                    | What does a flight from one city's streets, out over an ocean and into another city's streets, cost, with tiles arriving while it flies?   |
| `macro/navigate/zoom`                   | What does zooming from a city's streets out to the world and back in cost, with tiles arriving while it zooms?                             |
| `macro/update/geojson`                  | What does replacing a GeoJSON source's data cost, until the map shows the new data?                                                        |
| `macro/update/layers`                   | What does adding a batch of layers to a city's map, then removing them, cost until the map shows each?                                     |
| `macro/update/paint`                    | What does setting a layer's paint property cost, until the map shows the new value?                                                        |
| `macro/update/feature-state`            | What does a frame cost that moves a hover state from one feature to another?                                                               |
| `macro/update/images`                   | What does adding the images that visible symbols use cost, until the map shows them?                                                       |
| `macro/tile-server/street`              | What does a tile server's renderer spend on each new tile of a city, from zoom 7 to 14?                                                    |

Micro scenarios read what a map over the fixtures reads: Liberty's style, its
expressions, a sparse and a dense street tile as MVT and MLT, Tokyo's labels,
the glyph ranges and fonts they need, and Natural Earth's countries as GeoJSON.
`micro/draw` draws one layer type over the street tiles, the earthquakes or the
terrain tiles, with its paint constant, which its shader programs take as
uniforms, or read from each feature, which they take per vertex. A stage whose
cost grows with its input runs at two sizes, named by a last segment (`small`
and `large`, or `sparse` and `dense` for a tile), so that a change in how its
cost grows shows as a changed ratio between the two.

Macro scenarios drive a map over the fixtures one step at a time on the
scenario's own thread (see [Driving a map](#driving-a-map)), the core that the
runtime runs on its thread, with its background work on that thread too.
`macro/load/runtime` instead loads a map through `mln_runtime`, as an
application or the C API does: its thread, its pool's workers and an offscreen
render session, which interleave differently from run to run.

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
- `tiles/flight.pmtiles`: the world's tiles to zoom 4 with San Francisco's and
  Tokyo's to 14, in one archive, for a flight from one city to the other.
- `styles/liberty.json`: OpenFreeMap's Liberty style, its URLs turned into
  `asset://` URLs of the snapshot (and any layer it can't draw from the snapshot
  removed, as the manifest's `style_changes` says), with its sprite and the
  glyph ranges the tiles' labels need.
- `fonts/`: Noto Sans JP, whose outlines the glyph scenarios draw, and Noto
  Sans.
- `tiles/terrain.pmtiles`: Mapterhorn's elevation of central San Francisco,
  zooms 0 to 12, Terrarium-encoded WebP images of 512 pixels, for hillshading
  and color relief.
- `geojson/ne_10m_admin_0_countries.geojson`: Natural Earth's countries.
- `geojson/earthquakes-2026-10.geojson`: the USGS's 11,342 earthquakes of
  October 2026, for heatmaps and circles.

### Attribution and licenses

- Tiles: © [OpenStreetMap](https://www.openstreetmap.org/copyright)
  contributors, under the Open Database License 1.0, in the
  [OpenMapTiles](https://openmaptiles.org/) schema (CC BY 4.0), from
  [OpenFreeMap](https://openfreemap.org/)'s planet build. The MLT tiles are the
  same data, re-encoded.
- Style: OpenFreeMap's Liberty, MIT, from OSM Liberty and OSM Bright (code BSD
  3-Clause, design CC BY 4.0); its sprite's icons are Maki (CC0 1.0) and public
  domain. `styles/LICENSE.md` in the snapshot has the terms.
- Fonts and glyphs: Noto Sans and Noto Sans JP, SIL Open Font License 1.1, with
  their license files under `fonts/`.
- Countries: [Natural Earth](https://www.naturalearthdata.com/), public domain.
- Terrain: [© Mapterhorn](https://mapterhorn.com/attribution), from USGS 3DEP
  elevation models (public domain) around San Francisco and Copernicus GLO-30 at
  low zooms (© DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018,
  provided under COPERNICUS by the European Union and ESA, all rights reserved).
- Earthquakes: the [U.S. Geological Survey](https://earthquake.usgs.gov/),
  public domain.

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
fixtures aren't compared. A scenario fails, naming the command, when the
snapshot isn't installed. Data an application would supply, such as features to
add, is generated in code from a fixed seed instead.

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

Lower is better for every metric. Each is a **count** of work, a **time**, or a
count **per unit** of work.

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

A scenario adds its own counts and times, such as `frames`, `frames_incomplete`
(frames drawn with tiles missing), `tile_requests`, `gpu_buffers` (the bytes of
the GPU's vertex and index buffers), `gpu_objects` (the textures and buffers the
renderer made), `gpu_vertex_shader_invocations`, `gpu_primitives` and
`gpu_fragment_shader_invocations` (what the GPU's pipeline ran, where the device
counts it: WebGPU on Vulkan or D3D12, a CPU rasterizer such as lavapipe
included), or `background_work` (the times a frame started work off the frame,
such as laying out a tile). A scenario that repeats a unit of work, such as a
frame or a tile, also reports `instructions`, `allocations` and `allocated` per
unit, such as `instructions_per_frame`.

A user feels the costliest frames, not the average one: the frame that crosses
the antimeridian, or the one where a tile arrives. So a scenario whose measured
parts are each one unit of a user's experience (a frame, a tile, an edit) also
reports how the units spread, with no need to know which are costly:
`instructions`, `allocations`, `allocated` and `wall` of the median unit, the
90th percentile and the costliest, such as `allocations_per_frame_max`.

How closely a count repeats depends on what it counts. Allocations and bytes
repeat exactly in most scenarios. Instructions move by a few hundredths of a
percent between runs of one binary, but also with where the binary's code lies,
which an edit anywhere can change: by a tenth of a percent in most scenarios,
and by a few percent, up to ten, in some. That is real, the binary runs those
instructions, but it doesn't come from what the edit does. Counts that depend on
how several threads interleave, such as every count of `macro/load/runtime`,
move by more. Times depend on the machine and its load.

## Comparing

`mln-bench compare <ref>` checks the ref out in a Git worktree under
`target/bench-compare/tree` and lays this checkout's harness over it: all of
`tools/bench`, and the libraries' `bench` modules (see
[Reaching a library's internal stages](#reaching-a-librarys-internal-stages)).
So both sides run this checkout's scenarios on their own libraries. Both build
with this checkout's toolchain and `profile.toml`, and no path of either
checkout ends up in a binary, so the same source builds the same binary on both
sides. Each run reports its harness id, a hash of the harness's files and the
compiler, and the comparison refuses runs whose ids differ.

When this checkout's harness doesn't build against the ref, the comparison
stops. It never runs the ref's own harness instead. This happens when a change
adds a scenario or entry point that needs library code the ref lacks, or changes
the internals an entry point calls. Commit what the harness needs first, then
compare the change you want to measure with that commit
(`mise run bench:compare HEAD~1`).

### Counts

Each side runs once per scenario by default. The ref's runs are kept in
`target/bench-compare/samples`, by commit, and the ref runs again only until it
has three: the first comparison with a commit costs two extra runs per scenario,
later ones none. The ref is also built a second time with the harness's
`layout-probe` feature, which adds code that never runs and so moves the rest,
and each scenario runs once on that build.

A count's noise is the larger range of either side's runs; for instructions,
also the most that the layout probe of any ref compared so far moved them, since
one build of moved code shows too little of how far they move. The report shows
every count's change and noise, and marks a change over twice the noise, however
small: a count that repeats exactly is marked for any change. Whether a change
matters is the reader's call. A scenario with a marked count runs twice more on
this checkout, and its counts come from all three runs, so that a change that
doesn't repeat isn't marked.

### Times

Without `--rounds`, times show what one run of each side took, with no noise to
hold them against. `--rounds <n>` runs each side n times, interleaved ABBA so
that changes in the machine's load hit both alike. Each scenario's `wall` time
is then marked when a two-sided Mann-Whitney U test, Holm-adjusted over every
scenario compared, gives p < 0.05; its other times move with it. The fewest
rounds that can mark a time grow with the scenarios compared: 4 for one
scenario, 7 for all of them. Compare times only within one configuration.

Rounds need a quiet machine, and several sessions often measure on one. Every
`mln-bench` that measures holds one lock of the user's: runs that take only
counts share it and overlap freely, and rounds take it alone, waiting for the
measurements under way. The lock doesn't cover other sessions' builds and tests,
so rounds don't start while the machine's load average is over `--max-load` (a
quarter of its cores by default), and the report shows the load before and after
them.

### Environments

Each run records its environment: the OS, the CPU, how it counted instructions,
the GPU it drew on and its backend, whether that GPU is a CPU rasterizer, and
the fixtures it read. A scenario whose sides ran in different environments isn't
compared.

## Reading a report

```text
micro/expression/evaluate/liberty (counts: 3 base runs, 3 of this checkout)
  metric                         base     this  change  noise  moved
  instructions                 1.01 G   1.03 G   +2.2%  0.31%   more
  allocations                   687 k    699 k   +1.8%  0.00%   more
  retained_heap                23 KiB   23 KiB  +0.00%  0.00%
  wall                        39.4 ms  40.6 ms   +3.0%
  instructions_per_evaluation    1271     1299   +2.2%  0.31%   more
```

`base` is the ref's median, `this` this checkout's, `change` the difference as a
share of the base. `noise` is a count's noise as a share of the base, or with
rounds the tested time's adjusted p-value. `moved` marks a change beyond the
noise, `more` or `less` of the metric; lower is better for every metric. The
last line counts the marked changes; comparing a commit with itself gives none.

## Adding a scenario

A scenario answers one lasting question about one stage or operation, not one
pull request's claim; an exact expectation, such as that an idle map draws no
frames, belongs in a test.

A macro scenario measures what a user pays for: a style an application ships,
over real data, and all the work the operation causes, such as tiles loading,
layout, symbol placement and fades. The scenarios draw Liberty today; a style
whose costs differ, such as one of raster imagery or hillshading, comes as a
scenario of its own, its style and data added to the fixtures. An application's
own sources and layers go on top of the style, as `macro/update` adds them. A
scenario that narrows the style, or leaves out work the operation causes so as
to isolate one stage, measures a stage, not what a user feels: ask that question
in a micro scenario instead.

Add it to the module of its stage (`src/scenarios/micro/`) or operation
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
leave work between frames out. Measuring a part costs thousands of instructions
of its own, so a scenario of many small steps measures them as one part. `count`
and `bytes` declare the scenario's own metrics, and `per` the unit of work it
repeats, or `per_part` that each measured part is one unit, which adds how the
units spread. A macro scenario sets `bench.gpu` to the device it drew on, and
writes its last image to `bench.image` when that is set, outside its measured
parts. What more than one scenario uses goes in `src/support/`: the headless map
(`support::map`), the fixtures (`support::fixtures`) and seeded data
(`support::random`); what more than one micro scenario uses, such as the tiles
they read, in `src/scenarios/micro/`.

### Reaching a library's internal stages

A micro scenario calls a stage as a function over data. Where the stage is
internal to its library, the library has a `bench` feature, off by default, and
behind it a module `bench` (`src/bench.rs`) that is hidden from the
documentation: `mln_renderer::bench::TileLayout` runs a tile's layout job, for
instance. The harness turns the feature on in its `Cargo.toml`, so builds
without the harness leave the module out.

The `bench` modules are part of the harness: they count in its id, and a
comparison lays this checkout's over the ref's. An entry point takes and returns
the library's public types, wrapping internal ones, and calls the library's own
code for the stage rather than a copy of it, so that it measures what the
library runs and keeps building against other commits.

### Driving a map

`support::map::HeadlessMap` is a map on a headless frontend that runs only when
the scenario drives it, so that it does the same work on every run:

- Each frame is a tick of a 60 Hz frame clock that only frames advance, so
  symbol placement, fades and transitions don't depend on how long a frame took.
- The map's background work (file reads, tile layout, glyphs, the sprite,
  GeoJSON slicing) runs on a pool without threads, as in the browser:
  `HeadlessMap::load`, `settle` and `render_still` run all of it before each
  frame, so each frame has everything the frames before it asked for, and a map
  loads in the same steps, in the same order, on every run. A macro scenario's
  times are therefore the sum of its work, not what the runtime's four workers
  would take in parallel.
- `support::map::Frontend` holds the GPU device and target, made apart from the
  map, so that a scenario can leave them out of what it measures. A frame's part
  includes waiting for the GPU to finish it.
- `support::latency::Latency` holds a file source's answers back for a number of
  frames: a scenario that calls its `tick` before each frame, then runs the
  background work, sees each answer arrive the same number of frames after its
  request on every run.

### Repeating counts

A count repeats only if the process that counts it does the same work in every
run. The runner and the build see to it:

- Where the system places a process's code, stack and heap at random, the
  instructions that copies, comparisons and searches take change with the
  addresses, from one process to the next. A scenario's process
  (`mln-bench exec`) therefore runs again at fixed addresses where the system
  lets it, macOS and Linux (`src/aslr.rs`).
- The libraries' hash maps take a fixed seed in the benchmark build
  (`profile.toml`), where they otherwise seed each process from the clock.
- The harness compiles as one codegen unit, so that how the compiler splits and
  inlines the scenarios' code doesn't change with the size of libraries they
  don't call. Where the code lies still moves instructions, which a comparison
  measures with its layout probe instead.
- The first process of a newly built binary to run some code does more work than
  later ones: on macOS the system checks each page of the binary's code the
  first time a process reads it, and caches the GPU code it compiles for each
  binary. `run` and `compare` therefore run each scenario once on each binary
  before they measure, and drop what that run reports.
