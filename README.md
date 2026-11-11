# mln-rs

[![CI](https://github.com/louwers/mln-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/louwers/mln-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/mln-rs)](https://crates.io/crates/mln-rs)
[![docs.rs](https://img.shields.io/docsrs/mln-rs)](https://docs.rs/mln-rs)
[![Codecov](https://img.shields.io/codecov/c/github/louwers/mln-rs)](https://app.codecov.io/gh/louwers/mln-rs)

A Rust port of the [MapLibre Native](https://github.com/maplibre/maplibre-native)
map rendering core.

**[Open the demo](https://louwers.github.io/mln-rs/)** (in a browser with
WebGPU), the browser build in [examples/web](examples/web/README.md). On the
desktop, `just desktop` opens it in a window (examples/desktop).

## License

mln-rs is under the BSD 2-Clause License ([LICENSE.md](LICENSE.md)), as
MapLibre Native is. The code it ports from other projects keeps their
licenses: each package's `license` field names them, and the package ships
their texts. An app that ships mln-rs reproduces the notices of the crates
in its binary, which [cargo-about](https://github.com/EmbarkStudios/cargo-about)
collects. The FreeType License also asks for a credit in the app's
documentation.
