# mln-rs-actor

> **Warning: this is an internal crate of
> [mln-rs](https://crates.io/crates/mln-rs).** It is published only because the
> other mln-rs crates depend on it, and its API can change in any release
> without notice. mln-rs doesn't re-export it.

Ports the actor model and threading of the C++ core: `src/mln/actor` and
`util/{run_loop,async_task,thread,thread_pool}`, with the default platform's run
loop.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
