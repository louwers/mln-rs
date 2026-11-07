# Porting rules

Read `PLAN.md` first. The C++ source is in `../maplibre-native` (read-only for
everyone working in this repo).

## Porting

- **Port, don't redesign.** One Rust module per C++ file where practical, same
  type names (`RenderBackgroundLayer`, `TransformState`), methods in
  snake_case (`getZoom` → `get_zoom`), same algorithms in the same order.
- Start every file with the C++ file(s) it ports:
  `// Ports src/mln/renderer/layers/render_background_layer.cpp`.
- **Match C++ where it is observable** (render, query and expression tests
  with the C++ baselines; public API behaviour). Keep float types
  (`float` → `f32`) and the order of float operations, because they decide
  pixels. Port quirks that change those outcomes and comment them.
- **Don't reproduce** undefined behaviour, libstdc++ internals (sort tie
  order, container iteration order) or x86-64-specific conversion results,
  unless a test outcome depends on it. Record differences in `DEVIATIONS.md`.
- **Prefer a crate** when a comparison against the C++ (a difftest, run once)
  shows it doesn't change observable outcomes; port only when it does or
  when there is no crate. Ask the coordinator for new crates in
  `COORDINATION.md`.
- Difftests are a tool, not a gate for every function: write them where a
  crate or a port is in doubt, or to debug a render-test difference.
- Port only what the current milestone needs, but port those code paths
  completely. For a C++ branch you skip, leave a `// TODO(port): <what>`
  comment. Don't add `todo!()` on paths that run.
- No `unsafe` outside FFI and GPU buffer casts (use `bytemuck`).
- Errors: where C++ throws, return `Result` with the same message; where C++
  returns `optional`/`expected`, use `Option`/`Result`.

## Build and test

- Cargo, through the justfile where a task has a recipe (`just test`,
  `just fmt`, ...; `just` lists them). Wrap long commands in `timeout`.
- Each library is a package of the workspace (root `Cargo.toml`):
  `[dependencies]`, a `[[test]]` per integration test with its sources in
  `tests/`, test-only dependencies in `[dev-dependencies]` (in-repo ones by
  path, without a version). A new library needs the steps in RELEASING.md
  ("Adding a library"). CI's `release-crate` job checks the manifests.
- Format with `just fmt` (rustfmt, `rustfmt.toml`); leave generated
  files ("Do not edit") as their generator writes them, and put
  `#[rustfmt::skip]` on a new one's `mod` declaration. CI's `lint` job also
  runs clippy (`cargo clippy --workspace --all-targets`) and rustdoc with
  warnings as errors. The root `Cargo.toml` allows the clippy lints whose
  fixes would make the port read or behave less like the C++; for another
  such case, `#[allow]` it where it occurs and say why.
- Third-party crates: the ones in the root `Cargo.toml`'s
  `[workspace.dependencies]` (with their features). If you need another
  one, ask in `COORDINATION.md`; don't add it yourself.
- C++ test data (fixtures, the render, query and expression suites, the style spec): each package's `testdata/` holds copies of the files of maplibre-native its tests read, at their paths there. Copy the files a new test reads in the same way, and read them from `Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")`. They come from [johncarmack1984/maplibre-native@d171bf4](https://github.com/johncarmack1984/maplibre-native/commit/d171bf4b99343a27d0bfebf9ddf1ae79e5a616aa) (the globe and sky branches merged into [louwers/maplibre-native@4e03abe](https://github.com/louwers/maplibre-native/commit/4e03abe94cf562b10562c528578d109c7273694d), which is maplibre/maplibre-native 8e144d0 plus build changes), under its licenses. `just update-testdata [checkout]` copies them again from a maplibre-native checkout (by default `../maplibre-native`); the render, query and expression suites are copied whole, so new tests come along. Write outputs (actual and diff images) to `env!("CARGO_TARGET_TMPDIR")` in integration tests, or the system's temporary directory.
- GPU tests need an adapter: Metal on macOS, Vulkan on Linux (lavapipe
  through `VK_DRIVER_FILES` without a GPU). On NixOS, the justfile runs the
  tests and the apps inside the C++ repo's `#linux` dev shell.
- CI runs the justfile's `ci-*` recipes: `just ci-lint` checks the
  formatting, and runs clippy and rustdoc with warnings as errors;
  `just ci-coverage` runs the tests and writes their coverage for Codecov.

## Differential tests (`//difftest/<area>`)

Rust must behave exactly like C++. The difftests compare the released
crates with the C++ core, and live in
[mln-rs-difftests](https://github.com/louwers/mln-rs-difftests) (Bazel,
which builds the C++ core and the C++ references); its README describes
how to write one. To difftest an unreleased change, point that repository
at this one (its README says how). For a ported function:

- Pass POD values and `(ptr, len)` byte strings across the boundary, or JSON
  for structured values. C++ exceptions are caught in the wrapper and returned
  as error strings.
- Inputs: proptest strategies plus a hand-written list of edge cases
  (±0, NaN, ±inf, subnormals, boundaries, malformed strings).
- Compare bit-exactly (`to_bits`); any NaN equals any NaN.
- The oracle is built with C++ `assert`s live. Don't generate inputs that
  violate a C++ precondition; note the case instead.
- A comparison pinned to the Linux oracle (glibc libm/strtod, libstdc++
  ordering and hashing, x86-64 conversions, arm64 FMA contraction) runs on
  Linux only; leave genuinely cross-platform comparisons unrestricted — they
  run on macOS CI too.

## Ownership

Each agent edits only its own directories (see the PLAN.md workstream table).
Exceptions:

- `crates/mln_util/src`: any agent may **add** a new file for a C++ `util/`
  file it needs (for example `mat4.rs`) and append one `pub mod` /
  `pub use` line to `lib.rs`. Existing files belong to `expression`; ask in
  `COORDINATION.md` for changes to them. New files must not need new deps in
  `crates/mln_util/Cargo.toml` without asking first.
- `COORDINATION.md`: everyone appends; never rewrite others' entries.
- Shared files (the root `Cargo.toml`, `justfile`, `PLAN.md`, `PORTING.md`,
  `../maplibre-native`) belong to the coordinator. Request changes in `COORDINATION.md`.
- Don't commit. Others are editing the same working tree.

## Interfaces for M1

The M1 crates are written at the same time against each other's C++-shaped
APIs. These are the agreed shapes. Owners land compiling skeletons of them
early, so dependents can build, then fill them in.

- `mln_util::image` (runner): `PremultipliedImage` and `UnassociatedImage` as
  in `util/image.hpp` (`size: Size { width, height }`, `data: Vec<u8>` RGBA),
  plus PNG encode/decode as in `util/png_writer`/`png_reader`.
- `mln_util::size` (render): `Size { width: u32, height: u32 }`.
- `mln_platform_default` (render): `HeadlessFrontend::new(size, pixel_ratio)`,
  `HeadlessFrontend::render(&mut self, map: &mut Map) -> RenderResult`
  (`RenderResult { image: PremultipliedImage }`), as in
  `platform/default/include/mln/gfx/headless_frontend.hpp`.
- `mln_map` (render): `Map::new(frontend, observer, MapOptions)` with
  `MapOptions` builder methods as in `map_options.hpp` (`with_map_mode`,
  `with_size`, `with_pixel_ratio`), `Map::get_style_mut().load_json(&str)`,
  `Map::jump_to(&CameraOptions)`, `MapMode::{Continuous, Static, Tile}`.
- `mln_style` (style): `Style`, `style::Impl` holding
  `Vec<Immutable<layer::Impl>>` (`Immutable<T>` = `Arc<T>`), `BackgroundLayer`
  and its `Impl` with generated-style `BackgroundPaintProperties`
  (`Unevaluated`, `PossiblyEvaluated`, `Evaluated`), `Transitionable` →
  `Transitioning` → `PossiblyEvaluated` as in `style/properties.hpp`,
  `PropertyEvaluationParameters`, `TransitionOptions`. The renderer calls them
  as `render_background_layer.cpp` does.
- `mln_conversion` (expression): `Convertible` as in `style/conversion.hpp`
  and `conversion_impl.hpp` (`is_undefined`, `is_array`, `array_length`,
  `array_member`, `is_object`, `object_member`, `each_member`, `to_bool`,
  `to_number`, `to_double`, `to_string`, `to_value`), implemented for
  `serde_json::Value`, plus `conversion::Error { message }`.
- `mln_expression` (expression): `Expression` trait, `ParsingContext`,
  `ParseResult`, `type_::Type`, `EvaluationContext`, `EvaluationResult`,
  `Value`, `dsl` as in `style/expression/*`. `PropertyExpression<T>` and the
  legacy function conversion (`style/conversion/function.cpp`) are ported in
  `mln_style` on top of these.
- `mln_gfx` (gfx): the `gfx/` abstractions `render_background_layer.cpp` and
  `background_layer_tweaker.cpp` use on the drawable path: `Context`,
  `RendererBackend`, `HeadlessBackend`, `CommandEncoder`, `RenderPass`,
  `Drawable`, `DrawableBuilder`, `LayerGroup`/`TileLayerGroup`,
  `UniformBuffer(Array)`, `ShaderRegistry`/`ShaderProgramBase`, offscreen
  readback to `PremultipliedImage`. `mln_webgpu` implements them on `wgpu`.

When an interface needs to change, write the change in `COORDINATION.md`
before making it.
