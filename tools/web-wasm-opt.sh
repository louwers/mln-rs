# shellcheck shell=bash
# Sourced by the browser build's tasks, so that the module `web:size` measures
# is optimized as the one `web:build --profile release` ships.
#
# `web_wasm_opt <module> [flag]...` shrinks the module in place with wasm-opt
# -Oz, which inlines a function with one caller only when it is small. Its
# default, at any size, made the module bigger after brotli and slower to
# start. The flags go to wasm-opt: `--strip-debug` to ship, or `-g` to keep
# the function names. `web_wasm_opt_require` fails early, before a build, when
# wasm-opt isn't installed.

web_wasm_opt_require() {
    command -v wasm-opt > /dev/null ||
        { echo "error: wasm-opt not found: put binaryen's wasm-opt on the PATH (examples/web/README.md)" >&2; return 1; }
}

web_wasm_opt() {
    local module="$1"
    shift
    wasm-opt -Oz --converge --one-caller-inline-max-function-size=20 --zero-filled-memory --low-memory-unused \
        --strip-producers --strip-target-features "$@" "$module" -o "$module"
}
