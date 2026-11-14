# mln-rs

A Rust port of MapLibre Native's map rendering core.

## Project map

- `src/`: the `mln-rs` facade crate.
- `crates/`: the libraries, for native and the browser.
- `vendor/`: ports of MapLibre Native's third-party dependencies.
- `examples/`: apps that run mln-rs.
- `tools/`: tools for developing mln-rs.

## Dev tool commands

`mise install` installs the tools that mise.toml pins. `mise tasks` lists
every task. The justfile keeps the tasks' names from before they moved to
mise, each running its task. The common ones:

- `mise run test`: every test, including the render suite.
- `mise run fmt`: format with the pinned nightly rustfmt.
- `CI=true mise run ci:lint`: CI's lint job, warnings as errors.
- `mise run bench:compare <ref>`: benchmarks against another commit.
- `mise run web:build`, `mise run web:check`: build the browser build and its
  page, and compare it with native renders. `mise run web:dev` serves the
  page from its sources.

## Documentation

Describe the code as it is, for a reader who doesn't know its history.
Markdown docs give a high-level overview, not a tour of every feature and
change. Doc comments state contracts, and a code comment explains a
non-obvious choice where it is made. Don't narrate the implementation. Change
history, measurements and lists of callers go stale: put them in the commit
message and PR description, not the code.

## Pull requests

Open a PR as a draft until it's ready to merge.

A PR that claims a performance change includes measurements in its
description. A PR that changes what users see includes before and after
screenshots, attached with `gh pr edit --attach` or `gh pr create --attach`.
