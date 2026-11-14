#!/usr/bin/env just --justfile

# The dev tasks are mise tasks (`mise tasks` lists them). These recipes keep
# the names they had before the move, each running its mise task.

set positional-arguments := true

# The Cargo profile that the recipes that build with one pass on as
# `--profile`, e.g. `just profile=release web`
profile := 'dev'

@_default:
    just --list

# `mise run bench`
bench *args:
    mise run bench "$@"

# `mise run bench:android`
bench-android *args:
    mise run bench:android "$@"

# `mise run bench:compare`
bench-compare *args:
    mise run bench:compare "$@"

# `mise run bench:fixtures`
bench-fixtures *args:
    mise run bench:fixtures "$@"

# `mise run build`
build *args:
    mise run build "$@"

# `mise run test:c-abi`
c-abi-test *args:
    mise run test:c-abi "$@"

# `mise run c-web:build`
c-web *args:
    mise run c-web:build --profile {{profile}} "$@"

# `mise run c-web:check`
c-web-check *args:
    mise run c-web:check --profile {{profile}} "$@"

# Compile the workspace without building binaries
check *args:
    cargo check --workspace --all-targets "$@"

# `mise run check:32bit`
check-32bit *args:
    mise run check:32bit "$@"

# `mise run check:wasm`
check-wasm *args:
    mise run check:wasm "$@"

# `mise run ci:coverage`
ci-coverage *args:
    mise run ci:coverage "$@"

# `mise run ci:lint`
ci-lint *args:
    mise run ci:lint "$@"

# `mise run ci:package`
ci-package *args:
    mise run ci:package "$@"

# `mise run ci:msrv`
ci-test-msrv *args:
    mise run ci:msrv "$@"

# Remove the Cargo build
clean *args:
    cargo clean "$@"

# `mise run clippy`
clippy *args:
    mise run clippy "$@"

# `mise run coverage`
coverage *args:
    mise run coverage "$@"

# `mise run desktop`
desktop *args:
    mise run desktop --profile {{profile}} "$@"

# `mise run docs --open`, or `mise run docs` with the arguments given
docs *args:
    if [ $# -eq 0 ]; then mise run docs --open; else mise run docs "$@"; fi

# `mise run fix`
fmt *args:
    mise run fix "$@"

# `mise run generate:c-abi`
generate-c-abi *args:
    mise run generate:c-abi "$@"

# `mise run generate:icu-tables`
generate-icu-tables *args:
    mise run generate:icu-tables "$@"

# `mise run generate:style`
generate-style-code *args:
    mise run generate:style "$@"

# `mise run licenses`
licenses *args:
    mise run licenses "$@"

# `mise run package`
package *args:
    mise run package "$@"

# `mise run ratchet`
ratchet *args:
    mise run ratchet "$@"

# `mise run release`
release *args:
    mise run release "$@"

# `mise run test`
test *args:
    mise run test "$@"

# `mise run test:asan`
test-asan *args:
    mise run test:asan "$@"

# `mise run docs`
test-doc *args:
    mise run docs "$@"

# Check the formatting (dprint)
test-fmt *args:
    mise x -- dprint check "$@"

# `mise run test:msrv`
test-msrv *args:
    mise run test:msrv "$@"

# `mise run udeps`
udeps *args:
    mise run udeps "$@"

# Update the dependencies within their Cargo.toml requirements
update *args:
    cargo update "$@"

# `mise run update:ffi-abi`
update-ffi-abi *args:
    mise run update:ffi-abi "$@"

# `mise run update:style-spec`
update-style-spec *args:
    mise run update:style-spec "$@"

# `mise run update:testdata`
update-testdata *args:
    mise run update:testdata "$@"

# `mise run web:build`
web *args:
    mise run web:build --profile {{profile}} "$@"

# `mise run web:check`
web-check *args:
    mise run web:check --profile {{profile}} "$@"

# `mise run web:dev`
web-dev *args:
    mise run web:dev --profile {{profile}} "$@"
