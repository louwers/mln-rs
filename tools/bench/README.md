# mln-rs-bench

The benchmarks of mln-rs, and a comparison of two commits that says which
changes are real.

```sh
mise run bench                      # every case, once
mise run bench --filter Frame       # the cases whose name contains "Frame"
mise run bench:compare main         # this checkout, uncommitted changes included, against a Git ref
```

## Data

- The shared render suite's real-world styles (Chicago, San Francisco,
  Norway) at their tests' cameras, 512 x 512, and its CJK labels
  (`text-font/chinese`), read from its cache database.
- maplibre-native-ffi's basemap: VersaTiles' Shortbread tiles over central
  San Francisco (a 5 x 5 block at zoom 14) with Noto Sans glyphs and a
  generated sprite, 1024 x 768. `fixtures.json` and `style.json` are
  maplibre-native-ffi's, and the cases below that share its names use its
  cameras, so their numbers compare with the C++ core's. `mise run
  bench:fixtures` (`fixtures.py`) downloads the pinned files into
  `target/bench-fixtures` once; nothing else uses the network.
- The basemap's tiles as MLT, every polygon with the triangles mlt-core
  stores for MapLibre Native, which the harness encodes into the fixtures
  directory before the first case that reads them.

## Cases

| case | what one iteration is |
|---|---|
| `Still/<scene>` | a still frame of a loaded map, read back; the zoom alternates by 0.001, so each is a new frame over the same tiles |
| `Still/CollidingIcons/<n>` | as `Still`, of n icons at one point, each with its own sort key; placement hides all but one, or none in the `-overlap` case |
| `Load/Style/<scene>` | from loading the style on a new map to its first complete still; the GPU device is set up before. `basemap-mlt` is the basemap over its MLT tiles |
| `Frame/Throughput` | a camera jump around a 200-pixel circle until the GPU has finished the frame that shows it, over tiles an untimed lap loaded |
| `Frame/Paced` | a two-second flight from the block's north-west corner at zoom 14 to its south-east corner at zoom 15.5, past the tiles' zoom, on a 60 Hz display's clock, with only the first view loaded |
| `Frame/Oscillate` | three seconds of the bearing swinging between -90° and 90°, each swing half a second, as a rotate drag back and forth turns the map, on a 60 Hz display's clock, with only the first view loaded and every tile answered 100 ms late, as over a network |
| `Frame/OscillatePanZoom` | as `Frame/Oscillate`, with the center swinging 0.65 of a tile east and west while the zoom goes from 14.9 out to 14 at each end, as a pinch back and forth does |
| `Frame/Idle` | 100 ms of a settled map whose run loop draws whenever the map asks |
| `Frame/FeatureState` | a frame of `Frame/Throughput` over the basemap with its buildings and a street overlay colored by feature state, every building and street of the loaded tiles given a state |
| `Frame/FeatureStateHover` | a frame of `Frame/Throughput` over the basemap with its buildings colored by feature state, one building's state changing in every frame as hover highlighting does |
| `Frame/MapFeatureStateHover` | a frame of 100,000 GeoJSON points, each with a feature state set through the map, one point's state changing through the map in every frame |
| `Frame/HideLayers` | every layer of the loaded basemap hidden until the map settles, then shown again until it settles complete |
| `Frame/Extrusions`, `Frame/Extrusions/DataDriven` | a frame of the basemap's buildings extruded over its background, at a pitch, the zoom alternating as in `Still`; the buildings' height and color are constant, or come from their ids |
| `Frame/SettleWithoutSymbols` | from a camera nudge on the basemap with its symbol layers left out, which has nothing to place, until the map stops asking for frames |
| `Frame/AddImages` | from adding 50 icons to a settled basemap, five on each refresh, until it stops asking for frames; a symbol layer over 20 points draws five of them, which its tile asked for before they were added, and nothing draws the rest |
| `Load/Concurrent` | 16 renderers on threads of their own, each from loading a real-world style on a new map to its first complete still through `HeadlessFrontend::render`, as the render tests' workers draw; the GPU devices are set up before |
| `Load/GlyphCache/Private`, `Load/GlyphCache/Shared` | `Load/Concurrent` over the render suite's `text-font/chinese` style (500 labels, about 2,500 CJK characters over 80 glyph ranges), each renderer with a glyph cache of its own, or all sharing one `GlyphCache` |
| `Load/JumpWarm`, `Load/JumpCold` | a jump between two views that share no tile until a frame shows the new view complete, on the display's clock; cold drops the loaded tiles first |

In `Frame/Paced`, refreshes are absolute deadlines, and at each refresh
with no frame in flight the camera moves to where the flight will be at the
next refresh, so every run draws the same cameras however slow it is. A
frame is shown at the first refresh after the GPU finishes it, and the next
frame starts there. The `Frame/Oscillate` cases pace their frames the same
way. Their swings bring some tiles into view only for moments, shorter than
the 100 ms a tile takes to come; the file source answers each tile fresh for
an hour, as a tile server does, so the map fetches a tile again only after
it dropped it.

## Metrics

All are lower-is-better. A comparison gives a verdict on a case's **lead**
metrics only; `--verbose` also shows the others.

- **`time_ms`**, **`cpu_ms`**: an iteration's wall time, and the CPU time of
  every thread of the process, so work moved to the tile workers or a busy
  wait still counts. The median of the case's iterations.
- **`minstr`**, `mcycles`: the millions of instructions and of cycles every
  thread of the process retires in an iteration, the median of the case's
  iterations; macOS only. Other load on the machine hardly changes the
  instructions, so they show a change of the work done where `time_ms` and
  `cpu_ms` are noise; the cycles also count stalls, and vary with load and
  the cores the threads ran on.
- **`allocs`**, `alloc_kib`: heap allocations per iteration, of the whole
  process (the GPU driver's own allocations aren't Rust's and aren't
  counted).
- **`peak_heap_mib`**: the most heap live at once during the case.
- **`frame_p95_ms`**, **`frame_p99_ms`**: frame times, from asking for a
  frame until the GPU has finished it.
- In `Frame/Paced`: **`missed_pct`**, frames that finished after the next
  refresh; **`partial_pct`**, frames that drew a view with tiles still
  loading; **`longest_stall_ms`**, the longest time between two shown
  frames; **`complete_after_ms`**, how long after the flight's end the final
  view shows complete.
- In `Frame/Oscillate` and `Frame/OscillatePanZoom`: **`tile_requests`**,
  the tile requests of the swinging, **`partial_pct`** as in `Frame/Paced`,
  and `frame_max_ms`, the longest frame.
- In `Frame/Idle`: **`frames_per_s`**, frames drawn while nothing changes.
- In `Frame/SettleWithoutSymbols`: **`frames`**, frames drawn until the map stops asking for them.
- In `Frame/HideLayers`: **`hidden_heap_mib`** and **`hidden_gpu_mib`**, the heap and the GPU's vertex and index buffers live while the layers are hidden.
- In `Frame/Extrusions`: **`gpu_mib`**, the GPU's vertex and index buffers live once the view is complete.

## Comparing

`mise run bench:compare <ref>` checks the ref out in a worktree under
`target/bench-compare`, which the next comparison reuses, lays this
checkout's `tools/bench` over it so both sides run the same cases, and
builds it there. Then it runs one `mln-bench run` process per side per
round, in ABBA order so that drift hits both sides alike: `--rounds` (6)
processes per side. A process's value of a metric is one sample, since
iterations in one process share its caches and heap.

A case may use API that older commits lack. When this checkout's harness
doesn't build over the ref, the ref runs its own harness instead, the
report says so, and the cases only this checkout has show as "only in the
candidate". A case that changed between the two harnesses then compares
different work.

A lead metric gets a verdict when a two-sided Mann-Whitney U test,
Holm-corrected over the case's lead metrics, gives p < 0.05 and the medians
differ by at least `--threshold` percent (5). There are no verdicts when the
two sides ran on different GPUs. Then it compares the last frames of the
first round with pixelmatch, as the render tests do, and writes the
differences to `target/bench-compare/frames/diff`, so a side that got faster
by drawing less shows up.

Compare on an idle machine with a GPU; the report shows the load average
before and after, and a software rasterizer (lavapipe) is flagged. On a busy
machine, trust `minstr` and `allocs` over the times. To see how much your
machine's numbers vary, compare a commit with itself, which should give no
verdicts.

`--fail-on <metric>` makes the comparison fail, after its report, when that metric gets a worse verdict in any case. CI's `bench` workflow, run by hand on a branch, compares it with a base (main by default) this way on lavapipe, where the times are noisy but the allocations count the same as on any machine: it fails on a worse `allocs` or `peak_heap_mib`, and the report is in the job's summary. It takes about half an hour, so pull requests don't run it.

## Nightly

Every night, CI's `bench-nightly` workflow compares main with main as it was a day earlier, over every case, on lavapipe. When a case gets a worse verdict, `nightly.py` opens the issue "Nightly benchmarks: main got worse" with the report, or comments on it when it's open already, and a night without one closes it. The workflow runs only in louwers/mln-rs, and its `workflow_dispatch` takes another base.

## On an Android device

`mise run bench:android <ndk>` (`android.py`) compares the WebGPU and Vulkan
backends on the device adb reaches. It builds `mln-bench` for
`aarch64-linux-android` with the NDK's clang (`rustup target add
aarch64-linux-android` first), pushes it with the basemap fixture and the
cache database to `/data/local/tmp/mln-bench`, and runs it there from the
shell, headless: one process per backend per round (`--rounds`, 5) in ABBA
order, since a phone slows down as it warms up. `MLN_BENCH_CACHE_DB` tells
the binary where the cache database is. Without a `--filter`, it runs the
still cases, `Frame/Throughput`, `Frame/Paced` and `Load/Style/basemap`.

It prints each metric's median with its lowest and highest value over the
rounds, and leaves the reports and the last round's frames in
`target/bench-android`. `--pause <seconds>` lets the device cool before
each process; the battery's temperature is logged as each starts. `--keep`
leaves the pushed files on the device.

## Not measured yet

- GPU time apart from the frame (timestamp queries), and the frame's CPU
  time by stage: the port doesn't keep `RenderingStats` yet.
- Instructions and cycles on Linux and Windows.
- A synthetic clock for animations: in continuous mode the map reads the
  real clock, so fades and transitions in `Frame/Paced` follow wall time.
