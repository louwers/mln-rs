//! The render-test runner: renders the style tests of the render suite and
//! compares them with their expected images and results.
//!
//! The runner core reaches the map through [`runner::TestRunnerImpl`],
//! implemented over the real map in [`map`]. `--self-check` runs the full
//! comparison and reporting path without a map, with each test's expected
//! image as the rendered one.

pub mod file_source;
mod filesystem;
mod jobs;
mod log;
mod manifest_parser;
pub mod map;
mod metadata;
mod parser;
pub mod render_test;
pub mod runner;
mod style_ops;
mod test_runner_common;
mod watchdog;

pub use render_test::run_render_tests;

#[cfg(test)]
mod tests;
