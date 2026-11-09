# mln-rs: porting the MapLibre Native core to Rust

This repository is a **port** of the MapLibre Native C++ core
(`src/mln`, `include/mln` in `../maplibre-native`, about 167k lines). It is a
port, not a redesign:

- Modules, types, files and names carry over. Every Rust file starts with the
  C++ file(s) it ports, so a review is always a side-by-side diff.
- The architecture in `../maplibre-native/ARCHITECTURE.md` is kept: the Impl
  idiom, `Immutable<T>` snapshots diffed by the render objects, actor-based
  threading, the `gfx` backend abstraction.
- **Done means matching C++ where it is observable**: the shared render,
  query and expression suites with the C++ baselines, and the public API's
  behaviour. Quirks that change those outcomes are ported, not fixed
  (premultiplied `Color`, rapidjson number formatting in serialized
  expressions, CSS parsing); differences are commented where they are made.
- **Prefer a crate over a port** when a one-off comparison against the C++
  shows the difference does not change observable outcomes (`mlt-core`,
  `unicode-bidi`); port when it does (earcut) or when there is no crate.
  Differential tests are the measuring tool for that decision and for
  debugging, not a requirement for every function.
- **Don't reproduce** C++ undefined behaviour, one compiler's standard-library
  internals (libstdc++ sort tie order, x86-64 conversion results) or build
  configurations, unless a test outcome depends on it. Those already differ
  between the C++ builds on different platforms.
- Breadth before depth: the near-term goals are render-test conformance
  (headline metric: passes in the full render suite, `cargo test -p
  mln-rs-render-test --test render_tests`)
  and rendering OpenFreeMap's Liberty style completely (section 6). Stub
  specifics (exact BiDi, MLT, exotic options) until they block a test.

## 1. Build

- **Cargo** builds and tests everything (Rust edition 2024): the root
  `Cargo.toml` workspace, released to crates.io as the `mln-rs-*` packages
  behind `mln-rs` (RELEASING.md). Crates are listed in it; a new one also
  needs the steps in RELEASING.md ("Adding a library").
- Third-party crates are set in the root `Cargo.toml` workspace only.
- The C++ core is never linked into the port. The differential tests
  (`//difftest/<area>`) link it as the reference implementation; they live
  in [mln-rs-difftests](https://github.com/louwers/mln-rs-difftests), which
  builds the C++ core with Bazel and tests the released crates against it.
- Everything runs through the justfile (`just test`, ...). On NixOS, the
  tests and apps run inside the C++ repo's `#linux` flake shell.
- Test data comes from the C++ repo, copied into the packages' `testdata/`
  (`just update-testdata`) from the reference checkout: the
  `maplibre-native-next-support` branch of `louwers/maplibre-native`,
  currently `4e03abe94cf562b10562c528578d109c7273694d` (upstream
  `maplibre/maplibre-native` commit
  `8e144d0eca082e8350335b2172a6dfdc939b9cef` plus the changes that make its
  Bazel module usable as a dependency, which mln-rs-difftests needs).

## 2. Crate map

Measured with `tools/cycles/include_graph.py`: **26 of the 29 C++ modules form
a single include cycle** (only `math`, `i18n` and `programs` are acyclic).
That works for one C++ library but not for Cargo crates. The cycle is held
together by a few shared vocabulary headers plus a handful of `util/` files
that really belong to higher modules (`tools/cycles/back_edges.py` lists them
all). Cutting there gives this layering, bottom to top:

| Crate | Ports | Notes |
|---|---|---|
| `//vendor/geometry` | `mapbox::geometry` (geometry.hpp, in maplibre-native-base) | own types, not `geo-types`: the generic coordinate type matches C++ |
| `//vendor/feature` | `mapbox::feature` (`value`, which is `mapbox::base::Value`; `identifier`, `feature`) | the `u64`/`i64`/`f64` distinction in `Value` is load-bearing |
| `//vendor/csscolorparser` | `vendor/csscolorparser` | port of the C++ parser; the Rust crate of the same name accepts different input |
| `//vendor/rapidjson` | `rapidjson/internal/{dtoa,diyfp,ieee754}.h` only | number formatting for `util::toString(double)`; JSON itself is serde_json |
| `//vendor/pixelmatch` | the pixelmatch used by `render-test/` | image diff metric |
| `mln_math` | `math/` | |
| `mln_util` | `util/` minus the files below | `mat2/3/4`, `Color`, geo, `toString`, `interpolate` (generic part), image |
| `mln_tile_id` | `tile/tile_id.hpp`, `tile/geometry_tile_data.hpp` (interfaces) | cut point: included by util, style and expression |
| `mln_style_types` | `style/types.hpp`, `position`, `rotation`, `variable_anchor_offset_collection` | cut point: included by util and expression |
| `mln_conversion` | `style/conversion.hpp`, `conversion_impl.hpp`, generic `conversion/*` helpers | cut point: `Convertible` over serde_json; JNI/Swift front ends plug in later as in C++ |
| `mln_actor` | `actor/`, `util/{run_loop,thread,thread_pool}`, `platform/default` loop | std threads and channels, no async runtime |
| `mln_expression` | `style/expression/` minus `format_section_override` | own corpus: `metrics/integration/expression-tests` |
| `mln_style` | `style/` (layers, sources, properties, parser, light), `format_section_override` | the `Interpolator` impls for style types live here (orphan rule) |
| `mln_gfx` | `gfx/`, `shaders/` registry, `programs/` | backend-independent drawables, passes, uniform buffers |
| `mln_webgpu` | `webgpu/` | the first backend: on the `wgpu` crate, using the C++ backend's WGSL sources unchanged |
| `mln_renderer` | `renderer/`, `util/tile_cover` | |
| `mln_tile`, `mln_storage`, `mln_text`, `mln_layout`, `mln_sprite`, `mln_algorithm`, `mln_geometry` | the matching modules | phases 3-5 |
| `mln_map` | `map/`, `annotation/`, `layermanager/`, `util/{camera,action_journal}` | top of the core |
| `mln_platform_default` | `platform/default` (headless frontend and backend, file sources) | |
| `tools/render-test`, `tools/expression-test` | `render-test/`, `expression-test/` | run the shared suites |
| `//difftest/<area>` | | Rust vs. C++ differential tests |

Where the residual cycle in the middle (style, renderer, tile, text, layout)
cannot be cut without moving more than a few files, those modules share one
crate with a module tree that mirrors the directories. That decision is made
per pair when we get there, from the measured edges.

## 3. C++ → Rust idioms

| C++ | Rust |
|---|---|
| `variant<...>` (+ `recursive_wrapper`) | `enum` (+ `Box`), same variant order |
| `Immutable<T>`, `makeMutable`, pointer-identity diffing | `Arc<T>`, build-then-freeze, `Arc::ptr_eq` (no `Mutex`) |
| Impl idiom (`Layer` holding `Immutable<Layer::Impl>`) | same split; mutators clone, modify, freeze |
| `optional` / `expected` / `unique_ptr` / `shared_ptr` | `Option` / `Result` / `Box` / `Arc` |
| constructors that throw | `Result`, same error messages |
| observer interfaces | traits |
| `Actor`, `Mailbox`, `ActorRef`, `Scheduler`, `RunLoop` | same names; messages are closures or enums |
| rapidjson | serde_json behind `Convertible` |
| `std::min`/`std::max` on floats; `util::min`/`util::max` | `if b < a { b } else { a }`; `fmin`/`fmax` with glibc semantics; never `f64::min` where signed zeros matter |
| EJS style code generation | a Rust generator over the same `v8.json`, checked-in output |
| gtest `TEST(Group, Name)` | `#[test] fn name()` in `group.rs`, same fixtures |

## 4. Dependency map (vendor → Rust)

| C++ | Rust | Parity |
|---|---|---|
| **harfbuzz** (`text/harfbuzz*.cpp`, `HBShaper`) | **[HarfRust](https://github.com/harfbuzz/harfrust)** (`harfrust` 0.15, locally patched for two HarfRust bugs found by the difftest, to upstream), with the C++ build's `HB_TINY` configuration emulated (hidden AAT tables, no FeatureVariations, HarfBuzz 8.2 UCD tables) | measured: 94.2% of 59,015 shaping cases bit-identical over 149 fonts; the rest is HarfRust's Unicode 18 data vs. HarfBuzz 8.2's Unicode 15 (1,691 cases) and fallback mark positioning that `HB_TINY` disables and HarfRust cannot (1,649) |
| freetype (`text/freetype.cpp`: face loading, cmaps, rasterization) | the needed parts of FreeType 2.13 ported (face loading, cmap formats 0-14, charmap selection, the ftgrays rasterizer) plus hb-ft; outlines and hinting via `skrifa` | cmap lookups identical (19.1M); SDF step bit-identical; bitmaps 68% identical, 1.1% of pixels differ (skrifa vs. FreeType hinting) |
| csscolorparser, rapidjson dtoa, pixelmatch, earcut, kdbush, supercluster, geojson-vt, polylabel, wagyu | ported | exact |
| earcut: the `earcut` crate (georust) instead? | evaluated, rejected | `//difftest/earcut_crate`: identical without holes, but with holes only 25% identical and 1.1% of polygons cover a different area (earcut.js 3.x hole bridging); the port stays |
| maplibre-tile-spec C++ decoder (MLT) | the `mlt-core` crate (user decision) | assumed correct (user decision, 2026-11-04); the difftest against the C++ decoder was removed, and mln-rs-difftests' `MODULE.bazel` fetches `maplibre-tile-spec` (which mbgl-core links) from GitHub, so that vendor submodule is never needed |
| protozero + vector-tile | hand-rolled MVT reader | exact |
| sqlite | `rusqlite` (same engine) | exact; offline databases interchangeable both ways |
| nunicode (collation, case mapping) | ported (`//vendor/nunicode`, tables generated from the vendored sources) | exact: every code point compared against C++ |
| icu (number-format, BiDi) | number-format reimplemented for the locales the tests use; BiDi: `unicode-bidi` or a port, decided in M5 | number-format exact on the corpus; other locales differ |
| Dawn / wgpu-native (`webgpu/`) | `wgpu` 30 | WGSL from `include/mln/shaders/webgpu/*.hpp` used unchanged |
| boost, eternal, expected-lite, unordered_dense, unique_resource | std | |

## 5. Tests

1. **Differential tests** (`//difftest/<area>`, in mln-rs-difftests): a
   small `cc_library` on `@maplibre//:mbgl-core` exposes the C++ functions
   through `extern "C"`
   (POD and JSON strings across the boundary); a `rust_test` calls both
   implementations on proptest-generated and edge-case inputs and requires
   identical results. Comparisons are bit-exact; any NaN equals any NaN. They
   run on Linux x86-64 at `-c fastbuild` and `--config=oracle-opt`. A
   comparison that is pinned to the Linux oracle (glibc libm/strtod,
   libstdc++ ordering and hashing, x86-64 conversions, arm64 FMA contraction,
   or an oracle linking platform/linux) carries
   `target_compatible_with = ["@platforms//os:linux"]`, so wildcard runs skip
   it elsewhere; the rest (the text stack, MVT, geo, string, expression
   types, the vendored geometry fixtures) also run and pass on macOS/arm64,
   where `.bazelrc` builds the oracle with `-Wno-error`.
2. **Shared data suites**, from the copy of the C++ repo's `metrics/` in
   `tools/render-test/testdata` and `tools/expression-test/testdata`:
   - render tests (`integration/render-tests`, 184 groups) and query tests:
     same `style.json` files, fixtures, `expected*.png` and pixelmatch metric
     as the C++ runner. Baselines are never overwritten; a justified Rust
     difference gets an alternate expectation, as Metal and Vulkan already do;
   - expression tests (`integration/expression-tests`, 86 groups): the C++
     runner's comparison rules (6 significant digits, serialization round
     trip, `platform-all.json` ignores).
3. **Ported gtests** (`test/**`), test by test, same group and name, same
   fixtures. A script counts `TEST(` vs. `#[test]` per module to give a parity
   number.
4. **Benchmarks** (`benchmark/**`) via criterion, once subsystems exist.

## 6. Milestones

| | Milestone | Exit criterion |
|---|---|---|
| M0 | Scaffold | done: the C++ core builds as a dependency, seed crates pass |
| **M1** | **First Rust render tests** (done: 6 pass with zero difference, 2 ignored-failed pixel-identical to C++; with storage `background-opacity/color` passes too, and the full suite runs 108 passed / 0 failed) | `background-color/{default,literal,function,colorSpace-lab}` and `background-visibility/*` pass in the Rust render-test runner through Map → Style → RenderOrchestrator → RenderBackgroundLayer → gfx → `mln_webgpu` → headless readback, with the C++ runner's metric, baselines and ignore lists. `background-color/{colorSpace-hcl,transition}` are in the C++ `platform-all.json` ignores because C++ fails them; a faithful port reports them "ignored failed" too. `background-opacity/{color,overlay}` need the sprite and raster tiles from `metrics/cache-style.db` (storage: M4); `background-pattern/*` needs sprites (M3) |
| M2 | Expressions | expression corpus at the C++ pass rate; zoom/feature functions in paint properties |
| M3 | GeoJSON fill, line and circle | the `circle-*`, `fill-*` (non-pattern) and `line-*` render tests on GeoJSON sources pass; tile pipeline (sources, tile pyramid, workers) and buckets ported. The C++ `tile/` and `renderer/` include each other heavily (38 and 50 includes), so the high tile files live in `mln_renderer::tile` (PLAN section 2 rule); `mln_tile` stays the low decoding crate |
| M4 | Vector tiles and storage | MVT decoding difftests (done: 0 mismatches over 678,610 fixture features); local file and MBTiles sources; tile-based render tests (43 vector-tile line/fill tests, raster, `background-opacity/overlay`); MLT decoding (the C++ decoder is ~70k lines: separate decision) |
| M5 | Symbols and text | HarfRust shaping difftests; glyph PBFs; collision and placement; `text-*` and `icon-*` render tests |
| M6 | Rest of the style spec, query tests, platform shells | |

Status 2026-11-03 17:40 (verified): full shared render suite **1326 of 1460 passed, 0 failed**; all of Liberty's layer types render in `//platform/glfw:mln-glfw`. Next focus (user): unit-test parity.

Status 2026-11-04 (verified): the suite also runs on macOS, where wgpu
renders through Metal (`manifests/macos-wgpu.json`, selected by OS):
**1353 of 1460 passed, 0 failed** (79 ignored failed, 28 ignored passed).
The libraries' tests pass on macOS; most difftests stay Linux-only (the
oracle comparisons are pinned to Linux x86-64).

Near-term target (2026-11-03), **OpenFreeMap Liberty**: 111 layers
(background, raster PNG z0-6, 16 fill incl. 2 `fill-pattern`, 67 line incl. 17
`line-dasharray`, 25 symbol with glyph PBFs and sprite icons, 1
`fill-extrusion`), one OpenMapTiles vector source, all its expression ops
already ported. Remaining: online loading and the GLFW app (running), vector
tile pipeline and fill/line (running), line dashes, fill patterns, raster,
fill-extrusion, and symbols (layout, atlases, collision, placement), the
largest piece.

