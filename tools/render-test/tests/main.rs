// The integration tests, in one binary rather than one per file: each binary
// links the renderer and every backend, which makes many large ones.

mod suite;

// A query finds a layer where it draws.
mod query_coverage;

// --jobs against a run in one process.
mod jobs;
// Every test style's layers cross to the worker codec as they went in.
mod layer_round_trip;
// Every render and query test.
mod render_tests;
// The whole shared render suite with each test's expected image as the
// rendered one (`--self-check`).
mod self_check;
