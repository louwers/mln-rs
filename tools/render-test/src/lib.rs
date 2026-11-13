//! The render-test runner: renders the style tests of the render suite and
//! compares them with their expected images and results.
//!
//! The runner core reaches the map through [`runner::TestRunnerImpl`],
//! implemented over the real map in [`map`]. `--self-check` runs the full
//! comparison and reporting path without a map, with each test's expected
//! image as the rendered one.

pub mod file_source;
pub mod filesystem;
pub mod jobs;
pub mod log;
pub mod manifest_parser;
pub mod map;
pub mod metadata;
pub mod parser;
pub mod render_test;
pub mod runner;
pub mod style_ops;
pub mod test_runner_common;
pub mod watchdog;

pub use render_test::run_render_tests;

#[cfg(test)]
mod tests;
