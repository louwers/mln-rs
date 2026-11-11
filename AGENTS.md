# mln-rs

A Rust port of MapLibre Native's map rendering core.

## Project map

- `src/`: the `mln-rs` facade crate.
- `crates/`: the libraries, for native and the browser.
- `vendor/`: ports of MapLibre Native's third-party dependencies.
- `examples/`: apps that run mln-rs.
- `tools/`: tools for developing mln-rs.

## Dev tool commands

`mise install` installs the tools that mise.toml pins. `just --list` lists
every recipe. The common ones:

- `just test`: every test, including the render suite.
- `just fmt`: format with the pinned nightly rustfmt.
- `CI=true just ci-lint`: CI's lint job, warnings as errors.
- `just bench-compare <ref>`: benchmarks against another commit.
- `just web`, `just web-check`: build the browser build and its page, and
  compare it with native renders. `just web-dev` serves the page from its
  sources.

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
