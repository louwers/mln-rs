<p align="center">
  <img src="https://github.com/user-attachments/assets/7ff2cda8-f564-4e70-a971-d34152f969f0#gh-light-mode-only" alt="MapLibre Logo" width="200">
  <img src="https://github.com/user-attachments/assets/cee8376b-9812-40ff-91c6-2d53f9581b83#gh-dark-mode-only" alt="MapLibre Logo" width="200">
</p>

# mln-rs

[![CI](https://github.com/louwers/mln-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/louwers/mln-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mln-rs)](https://crates.io/crates/mln-rs)
[![docs.rs](https://img.shields.io/docsrs/mln-rs)](https://docs.rs/mln-rs)
[![Codecov](https://img.shields.io/codecov/c/github/louwers/mln-rs)](https://app.codecov.io/gh/louwers/mln-rs)
[![](https://img.shields.io/badge/Slack-%23maplibre--native--rust--port-2EB67D?logo=slack)](https://slack.openstreetmap.us/)

An experimental Rust port of
[MapLibre Native](https://github.com/maplibre/maplibre-native). See the
[onboarding discussion](https://github.com/maplibre/maplibre/issues/557) for the
motivation behind this project.

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

## Extensions

The built-in layer and source types go through the same contracts that a crate
outside mln-rs implements (the docs of `mln_rs::layer` and `mln_rs::source`
start there). A source type may give each tile a value of its own type, which
layer types written for it draw from directly. Three first-party extensions,
each a crate written against the `mln-rs` facade alone, add what a style may
declare, and an app adds them to the registry it starts its runtime with:

- [`mln-rs-ngon`](crates/mln_ngon): the `org.maplibre.ngon` layer type, a
  regular polygon at each point of a geometry source, which may collide with
  labels and with each other.
- [`mln-rs-location-puck`](crates/mln_location_puck): the
  `org.maplibre.location-puck` layer type, a device's location, heading and
  accuracy at one position, without a source.
- [`mln-rs-image-process`](crates/mln_image_process): named render targets that
  layers of any type draw into, and the `org.maplibre.image-process` layer type,
  which blurs, thresholds, colors and combines them.

The desktop and browser demos register all three.
