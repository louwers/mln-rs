#!/usr/bin/env bash
# Prints the packages that the cross-target tests lint and test, one per line
# (.config/mise/tasks/ci/target, .config/mise/tasks/device-farm/package): every
# library crate (root, crates/*, vendor/*) plus the expression and render test
# suites, without the desktop apps and the host-only CLI tools.
set -euo pipefail
cargo metadata --no-deps --format-version 1 --locked | jq -r '
    .workspace_root as $root |
    .packages[] |
    (.manifest_path | ltrimstr($root + "/")) as $rel |
    select(
        $rel == "Cargo.toml"
        or ($rel | startswith("crates/"))
        or ($rel | startswith("vendor/"))
        or $rel == "tools/expression-test/Cargo.toml"
        or $rel == "tools/render-test/Cargo.toml"
    ) |
    .name
'
