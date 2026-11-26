<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/cee8376b-9812-40ff-91c6-2d53f9581b83">
    <img src="https://github.com/user-attachments/assets/7ff2cda8-f564-4e70-a971-d34152f969f0" alt="MapLibre Logo" width="200">
  </picture>
</p>

# mln-rs

[![CI](https://github.com/maplibre/mln-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/maplibre/mln-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mln-rs)](https://crates.io/crates/mln-rs)
[![docs.rs](https://img.shields.io/docsrs/mln-rs)](https://docs.rs/mln-rs)
[![Codecov](https://img.shields.io/codecov/c/github/maplibre/mln-rs)](https://app.codecov.io/gh/maplibre/mln-rs)
[![](https://img.shields.io/badge/Slack-%23maplibre--native--rust--port-2EB67D?logo=slack)](https://slack.openstreetmap.us/)

An experimental Rust port of
[MapLibre Native](https://github.com/maplibre/maplibre-native). See the
[onboarding discussion](https://github.com/maplibre/maplibre/issues/557) for the
motivation behind this project.

## Getting Started

You need [rustup](https://rustup.rs) and [mise](https://mise.jdx.dev), which
installs the other tools and runs the dev tasks:

```sh
mise install       # the tools that .config/mise/config.toml pins
mise run desktop   # run the desktop demo app
mise run web:dev   # serve the browser demo at http://localhost:5173/
mise run test      # every test, including the render suite
```

`mise run desktop` and `mise run web:dev` default to the optimized `dev-opt`
profile with incremental compilation and debug checks. Use `--profile dev` for
unoptimized debugging or `--profile release` for shipping optimizations.

`mise tasks` lists every task.
