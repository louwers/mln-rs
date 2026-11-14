#!/usr/bin/env just --justfile

# The dev tasks are mise tasks (mise.toml and mise-tasks/; `mise tasks` lists
# them). Each recipe here runs the task of its name, passing its arguments on,
# for those used to running them with just.

set positional-arguments := true

# The Cargo profile that the recipes that build with one pass on as
# `--profile`, e.g. `just profile=release web`
profile := 'dev'

@_default:
    just --list

# Run the benchmarks (tools/bench/README.md), e.g. `mise run bench --filter Frame`
bench *args:
    mise run bench "$@"

# Compare the WebGPU and Vulkan backends on the Android device adb reaches, e.g. `mise run bench-android $ANDROID_NDK_HOME --rounds 8`
bench-android *args:
    mise run bench-android "$@"

# Compare the benchmarks of this checkout, uncommitted changes included, against a Git ref (tools/bench/README.md)
bench-compare *args:
    mise run bench-compare "$@"

# Download the benchmarks' basemap fixture into target/bench-fixtures, once (tools/bench/fixtures.py)
bench-fixtures *args:
    mise run bench-fixtures "$@"

# Build every package and test target
build *args:
    mise run build "$@"

# Run maplibre-native-ffi's ABI suite and exit programs against crates/mln_c
c-abi-test *args:
    mise run c-abi-test "$@"

# Build crates/mln_c's browser package for one backend (webgpu or gl) into target/c-web/<backend>
c-web *args:
    mise run c-web --profile {{profile}} "$@"

# Check crates/mln_c's browser package for one backend in headless Chromium
c-web-check *args:
    mise run c-web-check --profile {{profile}} "$@"

# Quick compile of the Cargo workspace without building a binary
check *args:
    mise run check "$@"

# Check that the C library builds for a 32-bit target (i686-unknown-linux-gnu), as CI does
check-32bit *args:
    mise run check-32bit "$@"

# Check that the libraries and examples/web build for wasm32-unknown-unknown, as CI does
check-wasm *args:
    mise run check-wasm "$@"

# Run the Cargo tests as CI does (Linux and macOS), writing their coverage as LCOV to target/llvm-cov/lcov.info for CI to upload to codecov.io
ci-coverage *args:
    mise run ci-coverage "$@"

# Run the checks of CI's lint job: formatting, the panic and RefCell-borrow ratchet, clippy, the API docs, the wasm32-unknown-unknown and 32-bit builds and the licenses, warnings as errors
ci-lint *args:
    mise run ci-lint "$@"

# Package every crate and compile every Cargo test target, as CI's release-crate job does
ci-package *args:
    mise run ci-package "$@"

# Check the Cargo workspace with the oldest Rust it declares, as CI's msrv job does
ci-test-msrv *args:
    mise run ci-test-msrv "$@"

# Clean the Cargo build
clean *args:
    mise run clean "$@"

# Run cargo clippy to lint the code. Arguments go to each `cargo clippy`
clippy *args:
    mise run clippy "$@"

# Measure the coverage of the tests (`mise run test`) from a clean start, then report it: by default, open the HTML report
coverage *args:
    mise run coverage "$@"

# Run the desktop app: a map in a window, with its controls (needs a display). Arguments go to the app
desktop *args:
    mise run desktop --profile {{profile}} "$@"

# Build the code documentation, as docs.rs does, and open it. Arguments go to `cargo doc` (default `--open`)
docs *args:
    mise run docs "$@"

# Print environment info
env-info *args:
    mise run env-info "$@"

# Reformat every Rust file with the pinned nightly rustfmt (MLN_NIGHTLY), installing it if needed. Arguments go to rustfmt, such as `--check`
fmt *args:
    mise run fmt "$@"

# Regenerate crates/mln_c's declarations (src/generated) from its C headers, with tools/c-abi-gen
generate-c-abi *args:
    mise run generate-c-abi "$@"

# Regenerate the ICU Bidi_Class table from an ICU 78.3 source tree's icu/source/common
generate-icu-tables *args:
    mise run generate-icu-tables "$@"

# Regenerate crates/mln_style's generated style code from the style spec (checked by style-codegen's generated_code_test)
generate-style-code *args:
    mise run generate-style-code "$@"

# Print a field of a package's Cargo metadata
get-crate-field *args:
    mise run get-crate-field "$@"

# Print the minimum supported Rust version (MSRV) of a package (mln-rs by default)
get-msrv *args:
    mise run get-msrv "$@"

# Check that every package in the dependency graph is under a license deny.toml allows
licenses *args:
    mise run licenses "$@"

# Package every crate from its packaged sources as the release does (RELEASING.md), and compile every test target
package *args:
    mise run package "$@"

# Fail when a file has more library panic sites or RefCell borrows than tools/ratchet/baseline.toml; `--update` lowers it
ratchet *args:
    mise run ratchet "$@"

# Run the release workflow on main: publish the crates at the version in Cargo.toml (RELEASING.md)
release *args:
    mise run release "$@"

# Run every test: the libraries' tests and the render, query and expression test suites. Arguments go to `cargo nextest run`
test *args:
    mise run test "$@"

# Run the runtime's, the Vulkan backend's and the C library's tests under AddressSanitizer, round after round, for `minutes`
test-asan *args:
    mise run test-asan "$@"

# Test documentation generation
test-doc *args:
    mise run test-doc "$@"

# Test code formatting
test-fmt *args:
    mise run test-fmt "$@"

# Check the Cargo workspace with the oldest Rust its packages declare (rust-version)
test-msrv *args:
    mise run test-msrv "$@"

# Find unused dependencies
udeps *args:
    mise run udeps "$@"

# Update the dependencies within their Cargo.toml requirements
update *args:
    mise run update "$@"

# Refresh the copies of maplibre-native-ffi's C headers and ABI suite from a checkout of it, and regenerate crates/mln_c's declarations
update-ffi-abi *args:
    mise run update-ffi-abi "$@"

# Refresh tools/style-codegen/v8.json from a tag or commit of the MapLibre style spec
update-style-spec *args:
    mise run update-style-spec "$@"

# Refresh the copies of maplibre-native's files in the packages' testdata/ from a checkout of it
update-testdata *args:
    mise run update-testdata "$@"

# Build the browser build (the map and its tile workers in Web Workers, and its font-face shaper) and its page into examples/web/dist
web *args:
    mise run web --profile {{profile}} "$@"

# Load render-suite views in headless Chromium and compare the frames with native renders (examples/web/README.md)
web-check *args:
    mise run web-check --profile {{profile}} "$@"

# Serve the browser build's page from its sources (examples/web/web) at http://localhost:5173/
web-dev *args:
    mise run web-dev --profile {{profile}} "$@"
