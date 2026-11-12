#!/usr/bin/env just --justfile

# The package that re-exports every library (the crates.io release).
main_crate := 'mln-rs'
# How to call the current just executable. Note that just_executable() may have `\` in Windows paths, so we need to quote it.
just := quote(just_executable())
# cargo-binstall needs a workaround due to caching when used in CI
binstall_args := if env('CI', '') != '' {'--no-confirm --no-track --disable-telemetry'} else {''}
# Where `just ci-coverage` writes the LCOV report that CI uploads to Codecov
coverage_lcov := 'target/llvm-cov/lcov.info'

# The C++ core the port is ported from, for the recipes that read its sources (update-testdata and the generate-*
# recipes).
cpp := justfile_directory() / '..' / 'maplibre-native'
# maplibre-native-ffi, whose C headers and ABI suite crates/mln_c and tools/c-abi-test copy (update-ffi-abi).
ffi := justfile_directory() / '..' / 'maplibre-native-ffi'

# The Cargo profile `just desktop` builds the app with, e.g. `just profile=release desktop`
profile := 'dev'

# On NixOS, the tests and the apps build and run inside flake.nix's dev shell, which has the C toolchain and the libraries they
# load (the Vulkan loader, Wayland, X11, ...). Elsewhere (macOS, CI) the system's libraries are the environment.
nixos := path_exists('/etc/NIXOS')
dev_shell := if nixos == 'true' {'nix develop ' + quote('path:' + justfile_directory() / '.nix-shell') + ' --command'} else {''}

# if running in CI, treat warnings as errors by setting CARGO_BUILD_WARNINGS to 'deny' unless it is already set
# Use `CI=true just ci-lint` to run the same checks as in GitHub CI.
# Use `just env-info` to see the current value of CARGO_BUILD_WARNINGS
ci_mode := if env('CI', '') != '' {'1'} else {''}
export CARGO_BUILD_WARNINGS := env('CARGO_BUILD_WARNINGS', if ci_mode == '1' {'deny'} else {'warn'})
export RUST_BACKTRACE := env('RUST_BACKTRACE', if ci_mode == '1' {'1'} else {'0'})

@_default:
    {{just}} --list

# Run the benchmarks (tools/bench/README.md), e.g. `just bench --filter Frame`
bench *args:  bench-fixtures _dev-shell
    {{dev_shell}} cargo run --release -q -p mln-rs-bench -- run {{args}}

# Compare the WebGPU and Vulkan backends on the Android device adb reaches, e.g. `just bench-android $ANDROID_NDK_HOME --rounds 8`
bench-android ndk *args:  bench-fixtures
    python3 -I tools/bench/android.py {{quote(ndk)}} {{args}}

# Compare the benchmarks of this checkout, uncommitted changes included, against a Git ref (tools/bench/README.md)
bench-compare base='main' *args:  bench-fixtures _dev-shell
    {{dev_shell}} cargo run --release -q -p mln-rs-bench -- compare {{quote(base)}} {{args}}

# Download the benchmarks' basemap fixture into target/bench-fixtures, once (tools/bench/fixtures.py)
bench-fixtures:
    python3 -I tools/bench/fixtures.py target/bench-fixtures

# Configures and builds tools/c-abi-test in target/c-abi-test-<backend>, against crates/mln_c's shared library, then runs it
# with tools/c-abi-test/run.py, whose arguments follow: `--update` rewrites the backend's list, `-v` prints each failure. The
# backend is MLN_C_TEST_BACKEND's: vulkan by default, whose list is expected-failures.txt, or opengl, on EGL, whose list is
# expected-failures-opengl.txt and which runs on llvmpipe as CI does with LIBGL_ALWAYS_SOFTWARE=1. Needs CMake, a C23 and
# C++23 compiler, the Vulkan headers and loader, a Vulkan driver such as lavapipe (VK_DRIVER_FILES), libwayland-server and
# libwayland-client, which the Wayland cases load, and for opengl the EGL and OpenGL ES headers and libraries.
[doc("Run maplibre-native-ffi's ABI suite against crates/mln_c, and compare its failures with the backend's list in tools/c-abi-test")]
[positional-arguments]
c-abi-test *args:  _dev-shell
    #!/usr/bin/env bash
    set -euo pipefail
    target="${CARGO_TARGET_DIR:-target}"
    backend="${MLN_C_TEST_BACKEND:-vulkan}"
    build="$target/c-abi-test-$backend"
    {{dev_shell}} cargo build -p mln-rs-c
    {{dev_shell}} cmake -S tools/c-abi-test -B "$build" -DMLN_C_LIBRARY="$PWD/$target/debug/libmaplibre_native_c.so" -DMLN_C_TEST_BACKEND="$backend"
    {{dev_shell}} cmake --build "$build" --parallel
    {{dev_shell}} python3 -I tools/c-abi-test/run.py --backend "$backend" "$build/mln_native_abi_tests" "$@"

# Build every package and test target
build:  _dev-shell
    {{dev_shell}} cargo build --workspace --all-targets

# Quick compile of the Cargo workspace without building a binary
check:
    cargo check --workspace --all-targets

# The browser build (wasm32-unknown-unknown, no atomics: one wasm instance per Web Worker, each with its own memory)
# needs the target, which rust-toolchain.toml installs. Clippy, so that examples/web, which only builds there, is linted
# too. Three runs, since one would unify their features: the libraries as published (mln_storage's `sqlite` feature
# on, its SQLite built through sqlite-wasm-rs, which needs a clang that targets wasm32, and the in-process font-face
# shaper), the browser build's two modules as `just web` builds them, which leave both out, and the map's module with
# its `webgl` feature. The check fails if anything turns SQLite back on in the browser build, or brings in a C build
# (`cc`) that would need that clang, or if the module that draws with WebGPU gets the OpenGL backend (mln_gl and its
# GL bindings), which only the `webgl` module needs.
[doc('Check that the libraries and examples/web build for wasm32-unknown-unknown, as CI does')]
check-wasm:  _dev-shell
    {{dev_shell}} cargo clippy --target wasm32-unknown-unknown -p {{main_crate}}
    cargo clippy --target wasm32-unknown-unknown -p mln-rs-web -p mln-rs-web-shaper
    cargo clippy --target wasm32-unknown-unknown -p mln-rs-web --features webgl
    ! cargo tree --target wasm32-unknown-unknown -p mln-rs-web --edges normal --prefix none | grep -q '^rusqlite '
    ! cargo tree --target wasm32-unknown-unknown -p mln-rs-web --edges normal,build --prefix none | grep -q '^cc '
    ! cargo tree --target wasm32-unknown-unknown -p mln-rs-web --edges normal --prefix none | grep -Eq '^(mln-rs-gl|glow) '

# Run the Cargo tests as CI does (Linux and macOS), writing their coverage as LCOV for CI to upload to codecov.io
ci-coverage:  env-info && (_coverage '--lcov' '--output-path' quote(coverage_lcov))
    rm -rf {{quote(parent_directory(coverage_lcov))}}
    mkdir -p {{quote(parent_directory(coverage_lcov))}}

# Run the checks of CI's lint job: formatting, the panic and TODO(port) ratchet, clippy, the API docs, the wasm32-unknown-unknown build and the licenses, warnings as errors
ci-lint:  env-info test-fmt ratchet clippy test-doc check-wasm licenses && assert-git-is-clean

# Package every crate and compile every Cargo test target, as CI's release-crate job does
ci-package:  env-info package && assert-git-is-clean

# Check the Cargo workspace with the oldest Rust it declares, as CI's msrv job does
ci-test-msrv:  env-info test-msrv && assert-git-is-clean

# Clean the Cargo build
clean:
    cargo clean

# Run cargo clippy to lint the code
clippy *args:
    cargo clippy --workspace --all-targets {{args}}

# Measure the Cargo tests' coverage and open the HTML report. Uses `cargo-llvm-cov`
coverage:  (_coverage '--open')

# Build and open code documentation
docs *args='--open':
    DOCS_RS=1 cargo doc --no-deps {{args}} --workspace

# Print environment info
env-info:
    @echo "Running for '{{main_crate}}' crate {{if ci_mode == '1' {'in CI mode'} else {'in dev mode'} }} on {{os()}} / {{arch()}}"
    @echo "PWD {{justfile_directory()}}"
    {{just}} --version
    rustc --version
    cargo --version
    rustup --version
    @echo "CARGO_BUILD_WARNINGS='$CARGO_BUILD_WARNINGS'"
    @echo "RUST_BACKTRACE='$RUST_BACKTRACE'"

# The nightly whose rustfmt `just fmt` (and CI's `test-fmt`) runs: its import options, imports_granularity and group_imports,
# are unstable. Pinned so every machine formats alike. Bump it on purpose, then run `just fmt`.
fmt_toolchain := 'nightly-2026-11-04'

# Generated files (their first lines say "Do not edit" or "@generated") stay as their generators write them: they are not passed to
# rustfmt, and their `mod` declarations carry #[rustfmt::skip], so formatting the module that declares them leaves them alone too.
[doc('Reformat every Rust file with the pinned nightly rustfmt, installing it if needed')]
fmt *args:
    #!/usr/bin/env bash
    set -euo pipefail
    files=()
    while IFS= read -r -d '' file; do
        head -n 5 "$file" | grep -q -e 'Do not edit' -e '@generated' || files+=("$file")
    done < <(git ls-files -z '*.rs')
    if ! rustup component list --toolchain {{fmt_toolchain}} --installed 2> /dev/null | grep -q '^rustfmt'; then
        rustup toolchain install {{fmt_toolchain}} --profile minimal --component rustfmt --no-self-update
    fi
    rustfmt +{{fmt_toolchain}} --config imports_granularity=Module,group_imports=StdExternalCrate {{args}} "${files[@]}"

# Needs libclang, which bindgen loads at run time (LIBCLANG_PATH names its directory if it isn't found).
[doc("Regenerate crates/mln_c's declarations (src/generated) from its C headers, with tools/c-abi-gen")]
generate-c-abi:
    cargo run -q -p mln-rs-c-abi-gen

# Regenerate the HarfBuzz Unicode tables (crates/mln_text's, and a difftest's) from the C++ core's HarfBuzz
generate-hb-ucd-tables:
    python3 -I crates/mln_text/tools/gen_hb_ucd.py {{quote(cpp)}} > crates/mln_text/src/hb_ucd_table.rs
    python3 -I crates/mln_text/tools/gen_hb_ucd.py {{quote(cpp)}} --full-gc > difftest/text/hb_ucd15_full_gc.rs

# crates/mln_text/tools/gen_icu_tables.py says where to get the ICU 78.3 sources.
[doc("Regenerate the ICU property tables (Bidi_Class, General_Category) from an ICU 78.3 source tree's icu/source/common")]
generate-icu-tables icu_common:
    python3 -I crates/mln_text/tools/gen_icu_tables.py bidi {{quote(icu_common)}} > crates/mln_text/src/icu/ubidi_props_data.rs
    python3 -I crates/mln_text/tools/gen_icu_tables.py gc {{quote(icu_common)}} > crates/mln_util/src/i18n/uchar_props.rs

# Regenerate vendor/nunicode's tables from the C++ core's vendored nunicode
generate-nunicode-tables:
    python3 -I vendor/nunicode/tools/gen_tables.py {{quote(cpp / 'vendor/nunicode')}} vendor/nunicode/src/tables

# The spec is tools/style-codegen/v8.json, a copy of the MapLibre style spec's (`just update-style-spec` refreshes it).
[doc("Regenerate crates/mln_style's generated style code from the style spec (checked by style-codegen's generated_code_test)")]
generate-style-code:
    cargo run -p mln-rs-style-codegen -- --spec tools/style-codegen/v8.json --out crates/mln_style/src

# Get a package field from the metadata
get-crate-field field package=main_crate:  (assert-cmd 'jq')
    @cargo metadata --no-deps --format-version 1 | jq -e -r '.packages | map(select(.name == "{{package}}")) | first | .{{field}} // error("Field \"{{field}}\" is missing in Cargo.toml for package {{package}}")'

# Get the minimum supported Rust version (MSRV) for the crate
get-msrv package=main_crate:  (get-crate-field 'rust_version' package)

# It builds with the Cargo profile in `profile`: `just profile=release desktop` runs an optimized build.
[doc('Run the desktop app: a map in a window, with its controls (needs a display). Arguments go to the app')]
[positional-arguments]
desktop *args:  _dev-shell
    {{dev_shell}} cargo run --profile {{profile}} -p mln-rs-desktop --bin mln-desktop -- "$@"

# The tools are mise.toml's (`mise install`); wasm-bindgen-cli must be the version in Cargo.lock. It builds with the Cargo
# profile in `profile`: `just profile=release web` for the page to ship, the dev profile to debug with the names kept. The
# release build also gets the release profile's fat LTO and one codegen unit, opt-level "s", and binaryen's wasm-opt
# shrinks it with -Oz and strips its names and other custom sections. It inlines a function with one caller only when that
# function is small: binaryen's default inlines them at any size, which made the module bigger after brotli and slower to
# start. Two modules, from one cargo run: the map and its tile workers (examples/web, mln_web), and the font-face shaper
# (crates/mln_web_shaper) that the map's Web Worker loads the first time a style declares font faces. A second cargo run
# builds the map's module again with its `webgl` feature, for browsers without WebGPU: it draws with WebGL2, and only
# those browsers download it. wasm-bindgen writes the modules into examples/web/web/pkg (the WebGL2 one into pkg/webgl),
# and Vite builds the page around them into examples/web/dist, every asset's name hashed by its content; serve that over
# HTTP, or run `just web-dev`.
[doc('Build the browser build (the map and its tile workers in Web Workers, and its font-face shaper) and its page into examples/web/dist')]
web:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ {{quote(profile)}} == release ]]; then
      # For this build only: the wasm32 target has its own directory, so native release builds keep theirs.
      export CARGO_PROFILE_RELEASE_OPT_LEVEL=s
    fi
    target="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/{{if profile == 'dev' {'debug'} else {profile} }}"
    pkg=examples/web/web/pkg
    if [[ {{quote(profile)}} != dev ]]; then
      command -v wasm-opt > /dev/null ||
        { echo "error: wasm-opt not found: put binaryen's wasm-opt on the PATH (examples/web/README.md)" >&2; exit 1; }
    fi
    # wasm-bindgen's output for a module cargo built, into a directory.
    bind() {
      wasm-bindgen --target web --out-dir "$2" "$target/$1.wasm"
      if [[ {{quote(profile)}} != dev ]]; then
        wasm-opt -Oz --converge --one-caller-inline-max-function-size=20 --zero-filled-memory --low-memory-unused \
          --strip-debug --strip-producers --strip-target-features "$2/$1_bg.wasm" -o "$2/$1_bg.wasm"
      fi
    }
    cargo build --profile {{profile}} --target wasm32-unknown-unknown -p mln-rs-web -p mln-rs-web-shaper
    bind mln_web "$pkg"
    bind mln_web_shaper "$pkg"
    # The same file again, with the feature.
    cargo build --profile {{profile}} --target wasm32-unknown-unknown -p mln-rs-web --features webgl
    bind mln_web "$pkg/webgl"
    pnpm -C examples/web install --frozen-lockfile
    pnpm -C examples/web build

# Vite's dev server, which reloads the page's scripts as they change; `just web` again rebuilds the modules.
[doc("Serve the browser build's page from its sources (examples/web/web) at http://localhost:5173/")]
web-dev:  web
    pnpm -C examples/web dev

# Offline, from the render suite's cache database. Needs Node 24, Playwright's Chromium (`npx playwright@1.61.1 install chromium`)
# and lavapipe (VK_DRIVER_FILES); builds the native render-test runner. CI's web job runs `just profile=release web-check`.
# Arguments go to examples/web/check/check.mjs, e.g. `just profile=release web-check --filter real-world`.
[doc('Load render-suite views in headless Chromium and compare the frames with native renders (examples/web/README.md)')]
[positional-arguments]
web-check *args:  web
    cargo build -p mln-rs-render-test --bin render-test
    node examples/web/check/check.mjs "$@"

# The desktop app is never released.
# The verify step serves the packaged crates to each other from a local registry, and Cargo
# treats a registry crate's name and version as immutable: it keeps the source it unpacked
# into CARGO_HOME and the build in target/ from the last run. Delete both, or a branch is
# verified against an older branch's copy of the in-repo crates at the same version.
[doc('Package every crate from its packaged sources as the release does (RELEASING.md), and compile every test target')]
package:
    rm -rf "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/mln-rs-* "${CARGO_TARGET_DIR:-target}"/debug/.fingerprint/mln-rs-*
    cargo package --workspace --locked --exclude mln-rs-desktop
    cargo test --workspace --locked --no-run

# Fail when a file has more library panic sites or TODO(port) comments than tools/ratchet/baseline.toml; `--update` lowers it
ratchet *args:
    python3 -I tools/ratchet/ratchet.py {{args}}

# Run the release workflow on main: publish the crates at the version in Cargo.toml (RELEASING.md)
release:
    gh workflow run release.yml --repo louwers/mln-rs --ref main

# The tests' headless frontends render with WebGPU, or with the backend MLN_RENDER_BACKEND names: webgpu, metal, vulkan,
# gl (OpenGL ES) or gl-desktop (desktop OpenGL).
# A test that hangs is ended after ten minutes (.config/nextest.toml).
[doc("Run every test: the libraries' tests and the render, query and expression test suites. Arguments go to `cargo nextest run`")]
[positional-arguments]
test *args:  _dev-shell (cargo-install 'cargo-nextest')
    {{dev_shell}} cargo nextest run --workspace "$@"

# Test documentation generation
test-doc:  (docs '')

# Test code formatting
test-fmt:  (fmt '--check')

# Check the Cargo workspace with the oldest Rust its packages declare (rust-version)
test-msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    msrv="$({{just}} get-msrv)"
    rustup toolchain install "$msrv" --profile minimal --no-self-update
    cargo "+$msrv" check --workspace --all-targets

# Check that every package in the dependency graph is under a license deny.toml allows. Uses `cargo-deny`
licenses:  (cargo-install 'cargo-deny')
    cargo deny --locked check licenses

# Find unused dependencies. Uses `cargo-udeps`
udeps:  (cargo-install 'cargo-udeps')
    cargo +nightly udeps --workspace --all-targets

# Update the dependencies within their Cargo.toml requirements
update:
    cargo update

# The ref is the maplibre/maplibre-style-spec tag (such as `v26.6.0`) or commit the copy is from. The default is a
# commit after v26.6.0 that replaces `emptyResponse` with vector sources' `sparse`. Run `just generate-style-code` after.
[doc("Refresh tools/style-codegen/v8.json from a tag or commit of the MapLibre style spec")]
update-style-spec ref='c2a460f6e55899e4b6e04b44f8e29e08fef3067b':  (assert-cmd 'curl')
    curl -fsSL https://raw.githubusercontent.com/maplibre/maplibre-style-spec/{{ref}}/src/reference/v8.json -o tools/style-codegen/v8.json

# The shared suites' directories are copied whole, so new tests come along; every other file that is there, outside
# tools/c-abi-test (update-ffi-abi), is copied again.
# The copies are from johncarmack1984/maplibre-native's maplibre-native-next-support-globe-sky branch at d171bf4b9934: the globe branch of maplibre/maplibre-native#4533 (phased/12h), with louwers/maplibre-native's maplibre-native-next-support branch at 4e03abe94cf5 (upstream 8e144d0eca08 plus what mln-rs-difftests needs) and the sky branch of maplibre/maplibre-native#4534 merged in.
[doc("Refresh the copies of maplibre-native's files in the packages' testdata/ from a checkout of it")]
update-testdata checkout=cpp:
    #!/usr/bin/env bash
    set -euo pipefail
    for dir in tools/render-test/testdata/metrics/{integration/render-tests,integration/query-tests,expectations/platform-linux,expectations/platform-macos,ignores,linux-gcc8-release} \
            tools/expression-test/testdata/metrics/integration/expression-tests; do
        rsync -a --delete {{quote(checkout)}}/"${dir#*/testdata/}"/ "$dir"/
    done
    git ls-files -z '*/testdata/*' ':!tools/c-abi-test/testdata' | while IFS= read -r -d '' file; do
        cp {{quote(checkout)}}/"${file#*/testdata/}" "$file"
    done
    echo "copied from {{checkout}} ($(git -C {{quote(checkout)}} describe --always --dirty 2>/dev/null || echo "not a git checkout"))"

# The headers go to crates/mln_c/include and the ABI suite, with the support files, plugin, fixtures and GPU fixtures it
# builds with, to tools/c-abi-test/testdata, each directory whole, so new files come along; then tools/c-abi-test/patches
# fix the suite's own bugs and drop its uses of the location-indicator layer type, which mln-rs doesn't have, and the
# declarations are regenerated. The copies are from maplibre/maplibre-native-ffi's t3/gpu-device-targets branch at
# 71435e96ed4b, which changes the render target headers ahead of their merge.
[doc("Refresh the copies of maplibre-native-ffi's C headers and ABI suite from a checkout of it, and regenerate crates/mln_c's declarations")]
update-ffi-abi checkout=ffi:
    #!/usr/bin/env bash
    set -euo pipefail
    rsync -a --delete {{quote(checkout)}}/include/maplibre_native_c/ crates/mln_c/include/maplibre_native_c/
    rsync -a --delete {{quote(checkout)}}/include/mln/ crates/mln_c/include/mln/
    cp {{quote(checkout)}}/include/maplibre_native_c.h {{quote(checkout)}}/LICENSE crates/mln_c/include/
    for dir in tests/native/abi tests/native/support tests/native/plugin tests/native/fixtures tests/graphics; do
        mkdir -p "tools/c-abi-test/testdata/$dir"
        rsync -a --delete {{quote(checkout)}}/"$dir"/ "tools/c-abi-test/testdata/$dir"/
    done
    cp {{quote(checkout)}}/LICENSE tools/c-abi-test/testdata/
    for patch in tools/c-abi-test/patches/*.patch; do
        git apply --directory=tools/c-abi-test/testdata "$patch"
    done
    {{just}} generate-c-abi
    echo "copied from {{checkout}} ($(git -C {{quote(checkout)}} describe --always --dirty 2>/dev/null || echo "not a git checkout"))"

# Measure the coverage of the tests (`just test`) from a clean start, then report it with the given arguments
_coverage *report_args:  _dev-shell (cargo-install 'cargo-llvm-cov') (cargo-install 'cargo-nextest')
    cargo llvm-cov clean --workspace
    {{dev_shell}} cargo llvm-cov nextest --no-report --workspace
    cargo llvm-cov report --workspace {{report_args}}

# The dev shell flake (flake.nix), used from a copy in .nix-shell: `nix develop` on the repository copies its tracked files into
# the store again after every change to one. Nothing to do outside NixOS.
_dev-shell:
    #!/usr/bin/env bash
    set -euo pipefail
    [[ "{{nixos}}" == true ]] || exit 0
    mkdir -p .nix-shell
    for file in flake.nix flake.lock; do
        cmp -s "$file" ".nix-shell/$file" || cp "$file" ".nix-shell/$file"
    done

# Ensure that a certain command is available
[private]
assert-cmd command:
    @if ! type {{command}} > /dev/null; then \
        echo "Command '{{command}}' could not be found. Please make sure it has been installed on your computer." ;\
        exit 1 ;\
    fi

# Make sure the git repo has no uncommitted changes. Fails if CI envvar is set.
[private]
assert-git-is-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "$(git status --porcelain --untracked-files=all)" ]; then
        >&2 echo "::error::git repo is not clean. Make sure compilation and tests artifacts are in the .gitignore, and no repo files are modified."
        if [[ "{{ci_mode}}" == "1" ]]; then
            >&2 echo "::group::git status"
            git status
            >&2 echo "::endgroup::"
            >&2 echo "::group::git diff (tracked changes)"
            git add . --intent-to-add
            git --no-pager diff
            >&2 echo "::endgroup::"
            exit 1
        else
            >&2 echo "git repo is not clean, but not failing because CI mode is not enabled."
        fi
    fi

# Check if a certain Cargo command is installed, and install it if needed
[private]
cargo-install $COMMAND $INSTALL_CMD='' $BINSTALL_EXTRA_ARGS='' *args='':
    #!/usr/bin/env bash
    set -euo pipefail
    unset CARGO_BUILD_WARNINGS
    if ! command -v $COMMAND > /dev/null; then
        echo "$COMMAND could not be found. Installing..."
        if ! command -v cargo-binstall > /dev/null; then
            set -x
            cargo install ${INSTALL_CMD:-$COMMAND} --locked {{args}}
            { set +x; } 2>/dev/null
        else
            set -x
            cargo binstall ${INSTALL_CMD:-$COMMAND} {{binstall_args}} $BINSTALL_EXTRA_ARGS --locked
            { set +x; } 2>/dev/null
        fi
    fi
