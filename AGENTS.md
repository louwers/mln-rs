# mln-rs

mln-rs started as a port of MapLibre Native, which stays a useful reference for
how maps should look and behave, but isn't a spec. Prefer idiomatic Rust and
good behavior over matching the C++ core's behavior, error messages or quirks.

The MapLibre Style Spec and render tests' expected images are the bar for
correctness, not the C++ or GL JS code.

## Project map

- `src/`: the `mln-rs` facade crate.
- `crates/`: the libraries, for native and the browser.
- `vendor/`: ports of MapLibre Native's third-party dependencies.
- `examples/`: apps that run mln-rs.
- `tools/`: tools for developing mln-rs.

## The C API

`crates/mln_c` holds the MapLibre Native C API's headers and implements them.
maplibre-native-ffi builds that crate and generates its language bindings from
the headers. Read `crates/mln_c/README.md` before you design, change or review
the C API.

## Dev tool commands

`mise install` installs the tools that .config/mise/config.toml pins.
`mise tasks` lists every task. The common ones:

- `mise run test`: every test, including the render suite.
- `mise run check`, `mise run fix`: the formatters and linters (.config/hk.pkl);
  `fix` formats with dprint and the pinned nightly rustfmt. `hk install --mise`
  adds a pre-commit hook that fixes the staged files, if you want one.
- `CI=true mise run ci:lint`: CI's lint job, warnings as errors.
- `mise run bench`, `mise run bench:compare <ref>`: the benchmarks, and this
  checkout's against another commit (tools/bench/README.md). They install the
  benchmarks' map data first, once per user (`mise run bench:fixtures`).
- `mise run web:build`, `mise run web:check`: build the browser build and its
  page, and compare it with native renders. `mise run web:dev` serves the page
  from its sources.

## Checking a change

Run the cheapest check that covers the change first, and widen it only as far as
the change reaches:

- The touched crate's tests: `cargo nextest run -p <package>`, such as
  `-p mln-rs-engine`, before the whole suite.
- The render tests the change affects: from `tools/render-test`, run the runner
  with your OS and backend's manifest and a regex over the tests' paths, e.g.
  `cargo run -p mln-rs-render-test -- --manifestPath manifests/macos-wgpu.json --filter "/render-tests/line-dasharray/" --jobs 0 --output <dir>`.
- `mise run test`, the whole suite, before a pull request.
- Performance: `mise run bench:compare <ref> -- --filter <scenario>`, read from
  its counts, one run per side, against their noise; the whole set (no filter)
  for a broad change. Run time rounds (`--rounds <n>`) only when a pull request
  claims a change in time.

A benchmark scenario answers a lasting question about a stage or an operation
(tools/bench/README.md); don't add one to prove one pull request's claim. An
exact expectation, such as "an idle map draws no frames", belongs in a test.

## Other targets

The mise environments `target-apple`, `target-android`, `target-ohos`,
`target-windows` and `target-freebsd` add the toolchains, SDKs and test devices
for Apple's phones and headsets, Android, OpenHarmony and HarmonyOS, Windows,
and FreeBSD. Select one per command (`mise -E target-android run ...`) or for
good with `env = ["target-android"]` in an untracked `.miserc.local.toml`;
`mise install` then installs what it needs. With one selected, Cargo builds for
its targets and runs their tests on this checkout's own simulator, emulator,
virtual machine or Wine prefix through
[devices-cli](https://github.com/sargunv/devices-cli), as in
`cargo nextest run --target aarch64-linux-android -p mln-rs-style`. On an x64
Linux host, the x86_64 targets of Android and OpenHarmony run on emulators and
FreeBSD's in a virtual machine, with KVM, and Windows' x64 targets under Wine.

Android's tests run in a Java VM's process, as an app's code does
(tools/android-test-host/README.md). They also run on real phones in AWS Device
Farm, and iOS's on a real iPhone, in an app (tools/ios-test-host/README.md): CI
runs them for `main`'s commits, and `mise run device-farm:run <platform>` from
your machine (tools/device-farm/README.md). Pull requests run them on the
emulator and the simulator.

A test that reads files of its package finds them through the
`CARGO_MANIFEST_DIR` that Cargo sets when it runs the test, not
`env!("CARGO_MANIFEST_DIR")`: on Android, OpenHarmony and iPhones the package is
a copy on the device.

## Documentation

Describe the code as it is, for a reader who doesn't know its history. Markdown
docs give a high-level overview, not a tour of every feature and change. Doc
comments state contracts, and a code comment explains a non-obvious choice where
it is made. Don't narrate the implementation. Change history, measurements and
lists of callers go stale: put them in the commit message and PR description,
not the code.

## Pull requests

Use Conventional Commits for commit and PR titles, and credit AI tools with
`Assisted-by:`, never as author, committer or `Co-authored-by:`. CI checks
commit titles and authorship; don't flag them in code review.

Open a PR as a draft until it's ready to merge.

A PR that could impact performance includes measurements in its description. A
PR that adds or removes significant or third party code includes Wasm size
measurements. A PR that changes what users see includes before and after
screenshots, attached with `gh pr edit --attach` or `gh pr create --attach`.
