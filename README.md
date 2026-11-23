# mln-rs

[![CI](https://github.com/louwers/mln-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/louwers/mln-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mln-rs)](https://crates.io/crates/mln-rs)
[![docs.rs](https://img.shields.io/docsrs/mln-rs)](https://docs.rs/mln-rs)
[![Codecov](https://img.shields.io/codecov/c/github/louwers/mln-rs)](https://app.codecov.io/gh/louwers/mln-rs)

A Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core.

**[Browser demo](https://louwers.github.io/mln-rs/)** (uses WebGPU on supported
browsers, otherwise WebGL2).

## Getting Started

You need [rustup](https://rustup.rs) and [mise](https://mise.jdx.dev), which
installs the other tools and runs the dev tasks:

```sh
mise install       # the tools that mise.toml pins
mise run desktop   # run the desktop demo app
mise run web:dev   # serve the browser demo at http://localhost:5173/
mise run test      # every test, including the render suite
```

`mise run desktop` and `mise run web:dev` default to the optimized `dev-opt`
profile with incremental compilation and debug checks. Use `--profile dev` for
unoptimized debugging or `--profile release` for shipping optimizations.

`mise tasks` lists every task.
